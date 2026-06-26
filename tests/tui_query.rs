mod common;

use common::{TestEnv, ok};

#[test]
fn top_level_help_describes_commands() {
    let env = TestEnv::new();
    let db = env.db("top-level-help.sqlite");
    let help = ok(env.aven(&db, ["--help"]));
    assert!(help.contains("Task commands:"));
    assert!(help.contains("add          Create a task"));
    assert!(help.contains("dep          Manage task dependencies"));
    assert!(help.contains("text         Safely edit long text fields"));
    assert!(help.contains("Project and label commands:"));
    assert!(help.contains("Setup and diagnostics:"));
    assert!(help.contains("--db <DB>                Use a specific SQLite database path"));
    assert!(help.contains("--workspace <WORKSPACE>  Use a specific workspace by name or key"));
}

#[test]
fn tui_command_is_available() {
    let env = TestEnv::new();
    let db = env.db("tui-help.sqlite");
    let help = ok(env.aven(&db, ["tui", "--help"]));
    assert!(help.contains("Usage: aven"));
    assert!(help.contains("--project"));
    assert!(help.contains("-p"));
}
