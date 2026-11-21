//! Dependency, source-purity and workspace-layout boundaries for runtime crates.

use serde_json::Value;
use std::{collections::BTreeSet, path::PathBuf, process::Command};

fn metadata(manifest: PathBuf) -> Value {
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
            "--locked",
            "--manifest-path",
        ])
        .arg(manifest)
        .output()
        .expect("run cargo metadata");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("metadata JSON")
}

const RUNTIME_CRATES: [&str; 12] = [
    "metron-protocol",
    "metron-agent",
    "metron-harness",
    "metron-store-sqlite",
    "metron-testkit",
    "metron-wasm-aot-loader",
    "metron-sandbox",
    "metron-mcp",
    "metron-provider-http",
    "metron-host",
    "metron-inference-local",
    "metron-inference-worker",
];
const RETIRED_CRATES: [&str; 6] = [
    "metron-core",
    "metron-app",
    "metron-lab",
    "metron-operators",
    "metron-adapters",
    "metron-cli",
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn root_workspace_members_and_dependency_boundaries() {
    let root = root().canonicalize().unwrap();
    let workspace = metadata(root.join("Cargo.toml"));
    let product_names: BTreeSet<_> = RUNTIME_CRATES.into_iter().collect();
    let packages = workspace["packages"].as_array().unwrap();
    let workspace_ids: BTreeSet<_> = workspace["workspace_members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap())
        .collect();
    let member_packages: Vec<_> = packages
        .iter()
        .filter(|p| workspace_ids.contains(p["id"].as_str().unwrap()))
        .collect();
    let member_names: BTreeSet<_> = member_packages
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        member_names,
        product_names
            .iter()
            .copied()
            .chain(["metron-architecture-tests"])
            .collect()
    );
    assert_eq!(workspace_ids.len(), RUNTIME_CRATES.len() + 1);
    assert_eq!(
        workspace["workspace_root"].as_str().unwrap(),
        root.to_str().unwrap()
    );
    assert!(
        !root.join("products/runtime/Cargo.toml").exists(),
        "old nested runtime workspace exists"
    );
    for name in RETIRED_CRATES {
        assert!(
            !root.join("crates").join(name).exists(),
            "retired laboratory crate {name} exists"
        );
    }
    for name in RUNTIME_CRATES {
        let old_directory = root.join("products/runtime/crates").join(name);
        assert!(
            !old_directory.join("src").exists(),
            "old nested source directory for {name} exists"
        );
        assert!(
            !old_directory.join("Cargo.toml").exists(),
            "old nested manifest for {name} exists"
        );
        let directory = root.join("crates").join(name);
        assert!(
            directory.join("src").is_dir(),
            "missing source directory for {name}"
        );
        let package = member_packages
            .iter()
            .find(|p| p["name"].as_str() == Some(name))
            .unwrap();
        assert_eq!(
            PathBuf::from(package["manifest_path"].as_str().unwrap())
                .canonicalize()
                .unwrap(),
            directory.join("Cargo.toml").canonicalize().unwrap(),
            "wrong manifest location for {name}"
        );
    }
    for package in &member_packages {
        let name = package["name"].as_str().unwrap();
        for dependency in package["dependencies"].as_array().unwrap() {
            let dep = dependency["name"].as_str().unwrap();
            let normal = dependency["kind"].is_null();
            assert!(
                !RETIRED_CRATES.contains(&dep),
                "package {name} depends on retired laboratory crate {dep}"
            );
            assert!(
                !normal || dep != "metron-testkit",
                "test support must not be a normal dependency of {name}"
            );
            assert!(
                !matches!(dep, "mistralrs" | "mistralrs-core") || name == "metron-inference-worker",
                "SDK dependency in {name}"
            );
        }
    }
    for package in member_packages
        .iter()
        .filter(|p| product_names.contains(p["name"].as_str().unwrap()))
    {
        let name = package["name"].as_str().unwrap();
        let allowed: &[&str] = match name {
            "metron-protocol" => &["serde", "serde_json", "sha2", "thiserror"],
            "metron-agent" | "metron-harness" => &["metron-protocol"],
            "metron-store-sqlite" => &["metron-protocol", "rusqlite"],
            "metron-testkit" => &["metron-protocol", "metron-harness"],
            // Verified model artifacts and the bounded compatibility tokenizer
            // are effect-boundary data; the SDK remains in the worker.
            "metron-inference-local" => {
                &["metron-protocol", "metron-harness", "sha2", "serde_json"]
            }
            "metron-inference-worker" => &[
                "metron-protocol",
                "metron-inference-local",
                "mistralrs",
                "mistralrs-core",
                "either",
                "tokio",
                "serde_json",
            ],
            "metron-wasm-aot-loader" => &[
                // Audited <80-line deserialization boundary: verifies F_SEAL_WRITE memfd,
                // BLAKE3, host tuple, and HMAC-SHA256 before Component::deserialize.
                "blake3", "hmac", "rustix", "sha2", "wasmtime",
            ],
            "metron-sandbox" => &[
                // Kernel isolation primitives and zero-Cranelift Lane A runtime host.
                "blake3",
                "landlock",
                "libc",
                "metron-wasm-aot-loader",
                "rustix",
                "seccompiler",
                "serde",
                "serde_json",
                "sha2",
                "thiserror",
                "wasmtime",
                "wasmtime-wasi",
            ],
            "metron-mcp" => &[
                // Typed rmcp 3.11 MCP bridge: routes every tool execution through
                // metron-sandbox and persists ResourceReceipts into metron-store-sqlite.
                "metron-protocol",
                "metron-sandbox",
                "metron-store-sqlite",
                "rmcp",
                "serde",
                "serde_json",
                "sha2",
                "thiserror",
                "tokio",
            ],
            "metron-provider-http" => &[
                // Pluggable OpenAI/Anthropic HTTP ModelProvider adapter.
                "metron-protocol",
                "reqwest",
                "serde",
                "serde_json",
                "sha2",
                "thiserror",
                "tokio",
            ],
            "metron-host" => &[
                // The composition root owns operational report serialization;
                // the inference SDK remains confined to the worker.
                "serde",
                "serde_json",
                // Safe Linux descriptors/sealed executable images belong only
                // at this operator/effect boundary, never in pure contracts.
                "rustix",
                "tokio",
                "metron-protocol",
                "metron-agent",
                "metron-harness",
                "metron-store-sqlite",
                "metron-testkit",
                "metron-sandbox",
                "metron-mcp",
                "metron-provider-http",
                "metron-inference-local",
            ],
            _ => panic!("unexpected package {name}"),
        };
        for dependency in package["dependencies"].as_array().unwrap() {
            let dep = dependency["name"].as_str().unwrap();
            if name == "metron-host"
                && matches!(
                    dep,
                    "rusqlite" | "blake3" | "wasmtime" | "wat" | "metron-wasm-aot-loader"
                )
            {
                assert_eq!(
                    dependency["kind"].as_str(),
                    Some("dev"),
                    "host may use {dep} only in tests"
                );
                continue;
            }
            if name == "metron-sandbox" && dep == "io-uring" {
                assert_eq!(
                    dependency["kind"].as_str(),
                    Some("dev"),
                    "metron-sandbox may use io-uring only in negative-control tests"
                );
                continue;
            }
            if name == "metron-sandbox" && dep == "wat" {
                let is_dev = dependency["kind"].as_str() == Some("dev");
                let is_opt = dependency["optional"].as_bool() == Some(true);
                assert!(
                    is_dev || is_opt,
                    "metron-sandbox may use wat only in dev or compiler feature"
                );
                continue;
            }
            if name == "metron-sandbox"
                && dep == "wasmtime"
                && dependency["kind"].as_str() == Some("dev")
            {
                continue;
            }
            if name == "metron-mcp"
                && matches!(
                    dep,
                    "blake3" | "wasmtime" | "wat" | "metron-wasm-aot-loader"
                )
                && dependency["kind"].as_str() == Some("dev")
            {
                continue;
            }
            if name == "metron-inference-worker"
                && matches!(dep, "mistralrs" | "mistralrs-core" | "either" | "tokio")
            {
                assert_eq!(
                    dependency["optional"].as_bool(),
                    Some(true),
                    "metron-inference-worker must keep {dep} strictly optional behind local-cuda/local-cpu"
                );
            }
            assert!(allowed.contains(&dep), "{name} must not depend on {dep}");
        }
    }
}

#[test]
fn pure_crates_do_not_use_ambient_io_or_lab_fixtures() {
    fn visit(path: &std::path::Path) {
        for entry in std::fs::read_dir(path).expect("read source directory") {
            let entry = entry.expect("directory entry");
            let path = entry.path();
            if path.is_dir() {
                visit(&path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let source = std::fs::read_to_string(&path).expect("read Rust source");
                for forbidden in [
                    "std::fs",
                    "std::net",
                    "std::process",
                    "std::env",
                    "std::time",
                    "fs::",
                    "net::",
                    "process::",
                    "env::",
                    "time::",
                    "io::",
                    "thread::",
                    "experiments/fixtures",
                ] {
                    assert!(
                        !source.match_indices(forbidden).any(|(offset, _)| {
                            offset == 0
                                || !source.as_bytes()[offset - 1].is_ascii_alphanumeric()
                                    && source.as_bytes()[offset - 1] != b'_'
                        }),
                        "{} contains {forbidden}",
                        path.display()
                    );
                }
            }
        }
    }
    for name in ["metron-protocol", "metron-agent", "metron-harness"] {
        visit(&root().join("crates").join(name).join("src"));
    }
}

#[test]
fn product_sources_do_not_reference_sealed_lab_fixtures() {
    fn visit(path: &std::path::Path) {
        for entry in std::fs::read_dir(path).expect("read source directory") {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                visit(&path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let source = std::fs::read_to_string(&path).expect("read Rust source");
                assert!(
                    !source.contains("experiments/fixtures"),
                    "{} references lab fixtures",
                    path.display()
                );
            }
        }
    }
    for name in RUNTIME_CRATES {
        visit(&root().join("crates").join(name));
    }
}

#[test]
fn unsafe_code_and_cranelift_boundaries_are_enforced() {
    let root = root().canonicalize().unwrap();

    // 1. metron-wasm-aot-loader must stay < 80 lines across its entire src/ tree.
    let loader_src = root.join("crates/metron-wasm-aot-loader/src");
    let mut loader_rs_files = Vec::new();
    for entry in std::fs::read_dir(&loader_src).expect("read loader src") {
        let p = entry.unwrap().path();
        if p.extension().is_some_and(|e| e == "rs") {
            loader_rs_files.push(p);
        }
    }
    assert_eq!(
        loader_rs_files.len(),
        1,
        "loader must stay a single micro-file"
    );
    let loader_lines = std::fs::read_to_string(&loader_rs_files[0])
        .unwrap()
        .lines()
        .count();
    assert!(
        loader_lines < 80,
        "metron-wasm-aot-loader exceeds <80-line cap: {loader_lines} lines"
    );

    // 2. No crate outside metron-wasm-aot-loader may allow unsafe_code or disable forbid(unsafe_code).
    for name in RUNTIME_CRATES {
        if name == "metron-wasm-aot-loader" {
            continue;
        }
        let crate_dir = root.join("crates").join(name);
        let manifest = std::fs::read_to_string(crate_dir.join("Cargo.toml")).unwrap();
        assert!(
            manifest.contains("workspace = true") || manifest.contains("unsafe_code = \"forbid\""),
            "{name} Cargo.toml must forbid unsafe_code"
        );
        fn check_no_allow_unsafe(dir: &std::path::Path) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let p = entry.unwrap().path();
                if p.is_dir() {
                    check_no_allow_unsafe(&p);
                } else if p.extension().is_some_and(|e| e == "rs") {
                    let src = std::fs::read_to_string(&p).unwrap();
                    assert!(
                        !src.contains("allow(unsafe_code)")
                            && !src.contains("unsafe_code = \"allow\""),
                        "{} permits unsafe_code outside metron-wasm-aot-loader",
                        p.display()
                    );
                }
            }
        }
        check_no_allow_unsafe(&crate_dir);
    }

    // 3. Production dependency graphs of runtime host & sandbox must contain zero Cranelift compiler,
    // and default metron-inference-worker must contain zero mistralrs SDK.
    for pkg in [
        "metron-wasm-aot-loader",
        "metron-sandbox",
        "metron-mcp",
        "metron-provider-http",
        "metron-host",
    ] {
        let out = Command::new("cargo")
            .current_dir(&root)
            .args([
                "tree",
                "-p",
                pkg,
                "--edges",
                "no-dev",
                "--offline",
                "--locked",
            ])
            .output()
            .expect("run cargo tree");
        assert!(
            out.status.success(),
            "cargo tree failed for {pkg}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let tree = String::from_utf8_lossy(&out.stdout);
        assert!(
            !tree.contains("wasmtime-internal-cranelift") && !tree.contains("cranelift-codegen"),
            "{pkg} runtime dependency tree unexpectedly pulls Cranelift compiler:\n{tree}"
        );
    }
    let worker_tree = Command::new("cargo")
        .current_dir(&root)
        .args([
            "tree",
            "-p",
            "metron-inference-worker",
            "--edges",
            "no-dev",
            "--offline",
            "--locked",
        ])
        .output()
        .expect("run cargo tree for worker");
    assert!(worker_tree.status.success());
    let worker_out = String::from_utf8_lossy(&worker_tree.stdout);
    assert!(
        !worker_out.contains("mistralrs"),
        "default metron-inference-worker must keep mistralrs gated behind local-cuda:\n{worker_out}"
    );
}
