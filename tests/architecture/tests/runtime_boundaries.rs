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

const RUNTIME_CRATES: [&str; 8] = [
    "metron-protocol",
    "metron-agent",
    "metron-harness",
    "metron-store-sqlite",
    "metron-testkit",
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
            ],
            "metron-host" => &[
                // The composition root owns operational report serialization;
                // the inference SDK remains confined to the worker.
                "serde",
                "serde_json",
                // Safe Linux descriptors/sealed executable images belong only
                // at this operator/effect boundary, never in pure contracts.
                "rustix",
                "metron-protocol",
                "metron-agent",
                "metron-harness",
                "metron-store-sqlite",
                "metron-testkit",
                "metron-inference-local",
            ],
            _ => panic!("unexpected package {name}"),
        };
        for dependency in package["dependencies"].as_array().unwrap() {
            let dep = dependency["name"].as_str().unwrap();
            if name == "metron-host" && dep == "rusqlite" {
                assert_eq!(
                    dependency["kind"].as_str(),
                    Some("dev"),
                    "host may use rusqlite only in tests"
                );
                continue;
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
                    "experiments/fixtures",
                ] {
                    assert!(
                        !source.contains(forbidden),
                        "{} contains {forbidden}",
                        path.display()
                    );
                }
            }
        }
    }
    for name in ["metron-protocol", "metron-agent"] {
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
