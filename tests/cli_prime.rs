mod common;

use std::fs;

use common::{TestEnv, command, contains_all, contains_none, extract_ref, ok};

#[test]
fn prime_prints_skill_primer_and_inferred_project_open_issues() {
    let env = TestEnv::new();
    let db = env.db("prime.sqlite");
    let repo = env.path("prime-app");
    init_git_repo(&repo);

    ok(aven_in_clean_git(
        &env,
        &db,
        &repo,
        ["label", "create", "bug"],
    ));
    let blocker = ok(aven_in_clean_git(
        &env,
        &db,
        &repo,
        [
            "add",
            "fix active issue",
            "--priority",
            "high",
            "--label",
            "bug",
        ],
    ));
    let blocker_ref = blocker.split_whitespace().nth(1).unwrap();
    let active = ok(aven_in_clean_git(
        &env,
        &db,
        &repo,
        ["add", "Fix active convention", "--label", "bug"],
    ));
    let active_ref = active.split_whitespace().nth(1).unwrap();
    ok(aven_in_clean_git(
        &env,
        &db,
        &repo,
        ["dep", "add", active_ref, blocker_ref],
    ));
    ok(aven_in_clean_git(
        &env,
        &db,
        &repo,
        ["update", active_ref, "--status", "active"],
    ));
    let done = ok(aven_in_clean_git(
        &env,
        &db,
        &repo,
        ["add", "finished issue"],
    ));
    let done_ref = done.split_whitespace().nth(1).unwrap();
    ok(aven_in_clean_git(
        &env,
        &db,
        &repo,
        ["update", done_ref, "--status", "done"],
    ));
    let canceled = ok(aven_in_clean_git(
        &env,
        &db,
        &repo,
        ["add", "canceled issue"],
    ));
    let canceled_ref = canceled.split_whitespace().nth(1).unwrap();
    ok(aven_in_clean_git(
        &env,
        &db,
        &repo,
        ["update", canceled_ref, "--status", "canceled"],
    ));

    let output = ok(aven_in_clean_git(&env, &db, &repo, ["prime"]));
    contains_all(
        &output,
        &[
            "# Aven CLI Primer",
            "## Issue Workflow",
            "aven update <ref> --status active",
            "aven note <ref> ...",
            "aven update <ref> --status done",
            "## Local Conventions",
            "Project: prime-app",
            "Open issue sample: 2",
            "Task titles: mixed lower-case and capitalized starts.",
            "Common statuses: active=1, inbox=1.",
            "Common labels: bug=2.",
            "## Open Issues",
            "Summary: total=2 active=1 ready=1 blocked=0",
            "Top blockers:",
            "### Active",
            "status=active labels=bug",
            "blocked_by=[",
            "title=\"Fix active convention\"",
            "### Ready",
            "status=inbox priority=high labels=bug blocks=[",
            "title=\"fix active issue\"",
            "### Blocked",
            "(none)",
        ],
    );
    contains_none(&output, &["finished issue", "canceled issue"]);
}

#[test]
fn prime_accepts_explicit_project() {
    let env = TestEnv::new();
    let db = env.db("prime-project.sqlite");
    ok(env.aven(&db, ["add", "app issue", "--project", "app"]));
    ok(env.aven(&db, ["add", "other issue", "--project", "other"]));

    let output = ok(env.aven(&db, ["prime", "--project", "app"]));
    contains_all(&output, &["# Aven CLI Primer", "Project: app", "app issue"]);
    contains_none(&output, &["other issue"]);
}

#[test]
fn prime_handles_no_current_project() {
    let env = TestEnv::new();
    let db = env.db("prime-none.sqlite");
    let cwd = env.path("plain");
    fs::create_dir_all(&cwd).unwrap();

    let output = ok(env.aven_in(&db, &cwd, ["prime"]));
    contains_all(
        &output,
        &[
            "# Aven CLI Primer",
            "No current project could be inferred. Run with --project <project>.",
        ],
    );
}

#[test]
fn prime_handles_no_open_issues() {
    let env = TestEnv::new();
    let db = env.db("prime-empty.sqlite");
    let repo = env.path("empty-app");
    init_git_repo(&repo);
    ok(aven_in_clean_git(
        &env,
        &db,
        &repo,
        ["project", "create", "empty-app"],
    ));

    let output = ok(aven_in_clean_git(&env, &db, &repo, ["prime"]));
    contains_all(
        &output,
        &[
            "# Aven CLI Primer",
            "## Local Conventions",
            "Project: empty-app",
            "Open issue sample: 0",
            "No open issues are available for convention summaries.",
            "## Open Issues",
            "No open issues.",
        ],
    );
}

fn aven_in_clean_git<I, S>(
    env: &TestEnv,
    db: &std::path::Path,
    cwd: &std::path::Path,
    args: I,
) -> std::process::Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = command();
    command
        .arg("--db")
        .arg(db)
        .env("XDG_STATE_HOME", env.state_dir())
        .env_remove("AVEN_SYNC_SERVER")
        .current_dir(cwd)
        .args(args);
    clean_git_env(&mut command);
    command.output().expect("run aven in cwd")
}

fn init_git_repo(repo: &std::path::Path) {
    fs::create_dir_all(repo).unwrap();
    let mut command = std::process::Command::new("git");
    command.args(["init", "-q"]).current_dir(repo);
    clean_git_env(&mut command);
    let output = command.output().expect("run git init");
    assert!(
        output.status.success(),
        "git init failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn clean_git_env(command: &mut std::process::Command) {
    for name in [
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CONFIG",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG_COUNT",
        "GIT_OBJECT_DIRECTORY",
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_IMPLICIT_WORK_TREE",
        "GIT_GRAFT_FILE",
        "GIT_INDEX_FILE",
        "GIT_NO_REPLACE_OBJECTS",
        "GIT_REPLACE_REF_BASE",
        "GIT_PREFIX",
        "GIT_SHALLOW_FILE",
        "GIT_COMMON_DIR",
    ] {
        command.env_remove(name);
    }
}

#[test]
fn prime_json_returns_structured_output_with_explicit_project() {
    let env = TestEnv::new();
    let db = env.db("prime-json.sqlite");
    ok(env.aven(&db, ["label", "create", "bug"]));
    let active_ref = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "active task",
            "--project",
            "app",
            "--priority",
            "high",
            "--label",
            "bug",
        ],
    )));
    ok(env.aven(&db, ["update", &active_ref, "--status", "active"]));
    let _inbox_ref = extract_ref(&ok(env.aven(&db, ["add", "inbox task", "--project", "app"])));

    let output = ok(env.aven(&db, ["prime", "--project", "app", "--json"]));
    let json: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(json["project"], "app");
    assert!(json["unavailable_reason"].is_null());
    assert!(json["open_issue_sample"].as_u64().unwrap_or(0) >= 2);
    assert!(!json["active"].as_array().unwrap().is_empty());
    assert!(!json["ready"].as_array().unwrap().is_empty());
    assert!(json["conventions"]["title_style"].is_string());
    assert!(json["conventions"]["statuses"].is_string());
}

#[test]
fn prime_json_returns_unavailable_without_project() {
    let env = TestEnv::new();
    let db = env.db("prime-json-none.sqlite");
    let cwd = env.path("plain");
    std::fs::create_dir_all(&cwd).unwrap();

    let output = ok(env.aven_in(&db, &cwd, ["prime", "--json"]));
    let json: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert!(json["project"].is_null());
    assert!(json["unavailable_reason"].is_string());
}

#[test]
fn prime_json_supports_limit() {
    let env = TestEnv::new();
    let db = env.db("prime-json-limit.sqlite");
    let _r1 = extract_ref(&ok(env.aven(&db, ["add", "task one", "--project", "app"])));
    let _r2 = extract_ref(&ok(env.aven(&db, ["add", "task two", "--project", "app"])));

    let output = ok(env.aven(&db, ["prime", "--project", "app", "--json", "--limit", "1"]));
    let json: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(json["open_issue_sample"], 1);
}
