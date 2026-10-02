//! Invariant 1: `metron-core` is pure, and dependencies point inward.
//!
//! The core may depend on serialisation and hashing crates only. No crate
//! other than the composition root may depend on the laboratory, the
//! adapters, or the operators; the application layer, the laboratory, the
//! operators and the adapters all depend on the core alone. None of the
//! inner crates may read files, the network, the process environment or
//! spawn threads.

use metron_architecture_tests::{crate_dir, find_tokens, manifest_deps, rust_sources};

const PURE_DEPS: &[&str] = &["serde", "serde_json", "sha2", "thiserror"];
const IO_TOKENS: &[&str] = &[
    "std::fs",
    "std::net",
    "std::process",
    "std::env",
    "std::io",
    "std::thread",
    "extern crate",
    "include_str!",
    "include_bytes!",
];

fn deps(name: &str) -> (Vec<String>, Vec<String>) {
    let manifest = crate_dir(name).join("Cargo.toml");
    (
        manifest_deps(&manifest, "dependencies"),
        manifest_deps(&manifest, "dev-dependencies"),
    )
}

#[test]
fn core_depends_only_on_serialisation_and_hashing() {
    let (deps, dev) = deps("metron-core");
    assert!(!deps.is_empty(), "core manifest parsed no dependencies");
    for d in deps.iter().chain(&dev) {
        assert!(
            PURE_DEPS.contains(&d.as_str()),
            "metron-core depends on `{d}`, which is not in the allow-list {PURE_DEPS:?}"
        );
    }
}

#[test]
fn core_sources_do_no_io() {
    let sources = rust_sources(&crate_dir("metron-core").join("src"));
    assert!(!sources.is_empty());
    let findings = find_tokens(&sources, &[IO_TOKENS, &["std::time"]].concat());
    assert!(
        findings.is_empty(),
        "core touches the outside world:\n{}",
        findings.join("\n")
    );
}

#[test]
fn inner_crates_depend_on_core_only() {
    for name in [
        "metron-app",
        "metron-lab",
        "metron-operators",
        "metron-adapters",
    ] {
        let (deps, dev) = deps(name);
        for d in deps.iter().chain(&dev) {
            if d.starts_with("metron-") {
                assert_eq!(
                    d, "metron-core",
                    "{name} depends on `{d}`; only metron-core is allowed"
                );
            }
        }
        assert!(
            deps.iter().any(|d| d == "metron-core"),
            "{name} must depend on metron-core"
        );
    }
}

#[test]
fn only_the_composition_root_sees_everything() {
    let (deps, _) = deps("metron-cli");
    for required in [
        "metron-core",
        "metron-app",
        "metron-lab",
        "metron-operators",
        "metron-adapters",
    ] {
        assert!(
            deps.iter().any(|d| d == required),
            "metron-cli must depend on {required}"
        );
    }
}

#[test]
fn app_lab_and_operators_do_no_io() {
    for name in ["metron-app", "metron-lab", "metron-operators"] {
        let sources = rust_sources(&crate_dir(name).join("src"));
        assert!(!sources.is_empty(), "{name} has no sources");
        let findings = find_tokens(&sources, IO_TOKENS);
        assert!(
            findings.is_empty(),
            "{name} touches the outside world:\n{}",
            findings.join("\n")
        );
    }
}
