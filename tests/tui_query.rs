mod common;

use common::{TestEnv, ok};

#[test]
fn top_level_help_describes_commands() {
    let env = TestEnv::new();
    let db = env.db("top-level-help.sqlite");
    let help = ok(env.aven(&db, ["--help"]));
    assert!(help.contains("TASKS"));
    assert!(help.contains("add          Create a task"));
    assert!(help.contains("list         List tasks"));
    assert!(help.contains("dep          Inspect and modify task dependencies"));
    assert!(help.contains("text         Get, diff, and set long text fields safely"));
    assert!(help.contains("bulk-update  Apply field updates across many tasks"));
    assert!(help.contains("WORKSPACE"));
    assert!(help.contains("project      Manage projects and their paths"));
    assert!(help.contains("SYNC"));
    assert!(help.contains("AGENTS"));
    assert!(help.contains("skill        Print a Claude Code skill primer"));
    assert!(help.contains("SETUP"));
    assert!(!help.contains("projects     List or search projects"));
    assert!(!help.contains("labels       List or search labels"));
    assert!(help.contains("--db <DB>                  Use a specific SQLite database path"));
    assert!(help.contains("--workspace <WORKSPACE>    Use a specific workspace by name or key"));
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
