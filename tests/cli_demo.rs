mod common;

use common::{TestEnv, fail, ok};

#[test]
fn demo_rejects_global_database_and_workspace_options() {
    let env = TestEnv::new();
    let db = env.db("must-not-exist.sqlite");

    let database_error =
        fail(env.aven_config(["--db", db.to_str().expect("utf8 database path"), "demo"]));
    assert!(database_error.contains("error demo-isolated option=--db"));
    assert!(!db.exists());

    let workspace_error = fail(env.aven_config(["--workspace", "personal", "demo"]));
    assert!(workspace_error.contains("error demo-isolated option=--workspace"));
    assert!(!db.exists());
}

#[test]
fn internal_demo_snapshot_materializes_current_demo_data() {
    let env = TestEnv::new();
    let snapshot = env.db("demo-snapshot.sqlite");

    let output = ok(env.aven_config([
        "internal",
        "demo-snapshot",
        "--output",
        snapshot.to_str().expect("utf8 database path"),
    ]));
    assert!(output.contains("Created demo snapshot"));
    assert!(snapshot.exists());

    let tasks = ok(env.aven(&snapshot, ["list", "--all", "--limit", "100"]));
    assert!(tasks.contains("Add due dates and scheduling"));

    let upcoming = ok(env.aven(&snapshot, ["search", "Parse natural-language dates"]));
    assert!(upcoming.contains("Parse natural-language dates when adding tasks"));

    let onboarding: serde_json::Value = serde_json::from_str(&ok(env.aven(
        &snapshot,
        [
            "search",
            "Update onboarding screenshots",
            "--limit",
            "1",
            "--json",
        ],
    )))
    .unwrap();
    let onboarding_ref = onboarding[0]["ref"].as_str().unwrap();
    let website: serde_json::Value = serde_json::from_str(&ok(env.aven(
        &snapshot,
        [
            "search",
            "Add terminal UI screenshots",
            "--limit",
            "1",
            "--json",
        ],
    )))
    .unwrap();
    let website_ref = website[0]["ref"].as_str().unwrap();
    assert_ne!(
        onboarding_ref.split_once('-').unwrap().0,
        website_ref.split_once('-').unwrap().0
    );

    let related: serde_json::Value = serde_json::from_str(&ok(
        env.aven(&snapshot, ["related", "list", onboarding_ref, "--json"])
    ))
    .unwrap();
    assert_eq!(related.as_array().unwrap().len(), 1);
    assert_eq!(related[0]["display_ref"], website_ref);
    assert_eq!(related[0]["title"], "Add terminal UI screenshots");

    let reciprocal: serde_json::Value = serde_json::from_str(&ok(
        env.aven(&snapshot, ["related", "list", website_ref, "--json"])
    ))
    .unwrap();
    assert_eq!(reciprocal[0]["display_ref"], onboarding_ref);
}

#[test]
fn demo_help_is_discoverable() {
    let env = TestEnv::new();
    let help = ok(env.aven_config(["--help"]));
    let interactive = help
        .split("INTERACTIVE")
        .nth(1)
        .expect("interactive help section")
        .split("AGENTS")
        .next()
        .unwrap();
    assert!(interactive.contains("demo"));
    assert!(interactive.contains("disposable sample tasks"));

    let command_help = ok(env.aven_config(["demo", "--help"]));
    assert!(command_help.contains("Explore aven with disposable sample tasks"));
}
