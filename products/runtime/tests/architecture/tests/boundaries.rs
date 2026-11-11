use serde_json::Value;
use std::{collections::BTreeSet, path::PathBuf, process::Command};

fn metadata(manifest: PathBuf) -> Value {
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
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

#[test]
fn workspaces_are_isolated_and_product_dependencies_follow_boundaries() {
    let runtime = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = runtime.join("../..");
    let product = metadata(runtime.join("Cargo.toml"));
    let lab = metadata(root.join("Cargo.toml"));
    let names = |v: &Value| -> BTreeSet<String> {
        v["packages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let product_names = names(&product);
    let lab_names = names(&lab);
    assert!(product_names.is_disjoint(&lab_names));
    assert_eq!(
        lab_names,
        [
            "metron-core",
            "metron-app",
            "metron-lab",
            "metron-operators",
            "metron-adapters",
            "metron-cli",
            "metron-architecture-tests"
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );
    assert_eq!(
        product_names,
        [
            "metron-protocol",
            "metron-agent",
            "metron-harness",
            "metron-store-sqlite",
            "metron-testkit",
            "metron-host",
            "metron-inference-local",
            "metron-inference-worker",
            "metron-runtime-architecture-tests"
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );
    for package in product["packages"].as_array().unwrap() {
        let name = package["name"].as_str().unwrap();
        let allowed: &[&str] = match name {
            "metron-protocol" => &["serde", "serde_json", "sha2", "thiserror"],
            "metron-agent" | "metron-harness" => &["metron-protocol"],
            "metron-store-sqlite" => &["metron-protocol", "rusqlite"],
            "metron-testkit" => &["metron-protocol", "metron-harness"],
            "metron-inference-local" => &["metron-protocol", "metron-harness", "sha2"],
            "metron-inference-worker" => &[
                "metron-protocol",
                "metron-inference-local",
                "mistralrs",
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
            "metron-runtime-architecture-tests" => &["serde_json"],
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
    for package in lab["packages"].as_array().unwrap() {
        for dep in package["dependencies"].as_array().unwrap() {
            assert!(
                !product_names.contains(dep["name"].as_str().unwrap()),
                "lab depends on product"
            );
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
    let runtime = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for name in ["metron-protocol", "metron-agent"] {
        visit(&runtime.join("crates").join(name).join("src"));
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
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../crates");
    visit(&crates);
}
