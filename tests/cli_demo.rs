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
