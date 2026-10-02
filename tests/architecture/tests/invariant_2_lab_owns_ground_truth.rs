//! Invariant 2: the laboratory owns ground truth. The system under study
//! cannot inspect hidden targets, evaluation code or promotion criteria.

use metron_architecture_tests::{crate_dir, find_tokens, item_text, manifest_deps, rust_sources};
use metron_lab::{HiddenFunction, PromotionGate, TruthTable};
use std::fs;

#[test]
fn hidden_function_is_sealed() {
    let source = fs::read_to_string(crate_dir("metron-lab").join("src/hidden.rs")).unwrap();
    let item =
        item_text(&source, "HiddenFunction").expect("HiddenFunction is defined in hidden.rs");
    for line in item.lines() {
        let t = line.trim();
        assert!(
            !(t.starts_with("pub ") && !t.starts_with("pub struct")),
            "HiddenFunction has a public field: {t}"
        );
    }
    assert!(
        !item.contains("Serialize") && !item.contains("Deserialize"),
        "HiddenFunction must not be serialisable: {item}"
    );
    let hidden = HiddenFunction::new(TruthTable::parse_rows(3, "00010111").unwrap());
    let shown = format!("{hidden:?}");
    assert!(shown.contains("<redacted>"), "Debug leaks: {shown}");
    assert!(
        !shown.contains("00010111"),
        "Debug leaks the table: {shown}"
    );
}

#[test]
fn promotion_criteria_are_private() {
    let source = fs::read_to_string(crate_dir("metron-lab").join("src/promotion.rs")).unwrap();
    let item = item_text(&source, "PromotionCriteria").expect("PromotionCriteria is defined");
    for line in item.lines() {
        let t = line.trim();
        assert!(
            !(t.starts_with("pub ") && !t.starts_with("pub struct")),
            "PromotionCriteria has a public field: {t}"
        );
    }
    let shown = format!("{:?}", PromotionGate::v0());
    assert!(
        shown.contains("<redacted>"),
        "Debug leaks criteria: {shown}"
    );
}

#[test]
fn nothing_but_the_composition_root_depends_on_the_lab() {
    for name in [
        "metron-core",
        "metron-app",
        "metron-operators",
        "metron-adapters",
    ] {
        let manifest = crate_dir(name).join("Cargo.toml");
        for section in ["dependencies", "dev-dependencies"] {
            let deps = manifest_deps(&manifest, section);
            assert!(
                !deps.iter().any(|d| d == "metron-lab"),
                "{name} must not depend on metron-lab ({section})"
            );
        }
    }
}

#[test]
fn the_system_has_no_judge_and_reads_no_fixtures() {
    for name in ["metron-core", "metron-app", "metron-operators"] {
        let sources = rust_sources(&crate_dir(name).join("src"));
        let findings = find_tokens(
            &sources,
            &["fn judge", "fixture", "experiments/", "HiddenFunction"],
        );
        assert!(
            findings.is_empty(),
            "{name} reaches for laboratory property:\n{}",
            findings.join("\n")
        );
    }
    let lab = rust_sources(&crate_dir("metron-lab").join("src"));
    assert!(
        !find_tokens(&lab, &["fn judge"]).is_empty(),
        "the laboratory must define the judge"
    );
}
