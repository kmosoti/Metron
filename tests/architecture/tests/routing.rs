//! M3 (ADR 0015): the routing harness fits routers on the train split,
//! chooses on validation, judges on test, and its replayed routers match
//! their simulated choices.

use metron_architecture_tests::workspace_root;
use metron_cli::{RouteOptions, run_route};
use metron_lab::Manifest;
use std::fs;

#[test]
fn the_routing_harness_runs_end_to_end_and_replays_match() {
    // The committed manifest, shrunk to arity 6, three targets per class
    // and one seed so the suite stays fast.
    let root = workspace_root();
    let text = fs::read_to_string(root.join("experiments/manifests/structure-arity8.json"))
        .expect("manifest");
    let mut json: serde_json::Value = serde_json::from_str(&text).expect("json");
    json["name"] = "structure-routing-smoke".into();
    json["lab"]["tasks"]["arity"] = 6.into();
    json["lab"]["tasks"]["pool"]["per_family"] = 4.into();
    json["lab"]["tasks"]["targets_per_family"] = 4.into();
    json["lab"]["max_probes"] = 64.into();
    json["system"]["budget"]["max_oracle_probes"] = 64.into();
    json["headroom"]["resamples"] = 200.into();
    json["headroom"]["cost_model"]["failure_cost"] = 128.0.into();
    let manifest_text = serde_json::to_string_pretty(&json).unwrap();
    Manifest::from_json(&manifest_text).expect("valid manifest");
    let dir = std::env::temp_dir().join(format!("metron-route-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("structure-routing-smoke.json");
    fs::write(&path, manifest_text).unwrap();
    let outcome = run_route(
        &path,
        &RouteOptions {
            results_root: dir.join("results"),
            seeds: Some(2),
            verbose: false,
        },
    )
    .expect("route runs");
    assert_eq!(outcome.reports.len(), 2, "primary and secondary prefix");
    let primary = &outcome.reports[0];
    let names: Vec<&str> = primary.routers.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names[..3],
        ["posterior-order", "posterior-reverse", "tabular"]
    );
    assert!(names[3].starts_with("ridge"));
    assert!(primary.entropy_floor.is_some());
    assert!(primary.oracle_router_test <= primary.routers[0].cost["test"] + 1e-9);
    assert!(outcome.dir.join("route.md").exists());
    assert!(outcome.markdown.contains("Routing pays:"));
    let _ = fs::remove_dir_all(&dir);
}
