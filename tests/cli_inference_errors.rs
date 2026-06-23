mod common;

use std::fs;
use std::process::Command;

use common::{TestEnv, command, contains_all, contains_none, fail, ok};

#[test]
fn infers_project_from_path_mapping() {
    let env = TestEnv::new();
    let db = env.db("mapping.sqlite");
    let mapped = env.path("mapped");
    let nested = mapped.join("sub");
    fs::create_dir_all(&nested).unwrap();
    env.write_config(&format!(
        r#"local:
  db_path: "{}"
"#,
        db.display()
    ));

    let create = ok(env.aven_config([
        "project",
        "create",
        "mapped",
        "--path",
        mapped.to_str().unwrap(),
    ]));
    contains_all(&create, &["created-project mapped"]);
    let config = fs::read_to_string(env.config_file()).unwrap();
    contains_all(
        &config,
        &[
            "# aven-managed project path mapping",
            "workspace: default",
            "project: mapped",
            mapped.to_str().unwrap(),
        ],
    );

    let output = ok(aven_config_in(&env, &nested, ["add", "mapped inference"]));
    contains_all(&output, &["project=mapped"]);
}

#[test]
fn infers_project_from_git_root() {
    let env = TestEnv::new();
    let db = env.db("git.sqlite");
    let repo = env.path("git-inferred");
    init_git_repo(&repo);

    let output = ok(env.aven_in(&db, &repo, ["add", "git inference"]));
    contains_all(&output, &["project=git-inferred", "created G"]);
}

#[test]
fn infers_project_from_main_worktree_for_linked_worktree() {
    let env = TestEnv::new();
    let db = env.db("linked-worktree.sqlite");
    let repo = env.path("main-project");
    let linked = env.path("linked-checkout");
    init_git_repo(&repo);
    ok_git(
        clean_git_command()
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(&repo),
    );
    ok_git(
        clean_git_command()
            .args([
                "worktree",
                "add",
                "--detach",
                linked.to_str().unwrap(),
                "HEAD",
            ])
            .current_dir(&repo),
    );

    let output = ok(env.aven_in(&db, &linked, ["add", "linked inference"]));
    contains_all(&output, &["project=main-project"]);
}

#[test]
fn project_path_commands_preserve_config_comments() {
    let env = TestEnv::new();
    let db = env.db("comments.sqlite");
    let mapped = env.path("commented");
    fs::create_dir_all(&mapped).unwrap();
    env.write_config(&format!(
        r#"# top comment
local:
  # db comment
  db_path: "{}"

project:
  # project comment
  overrides:
    # manual override comment
    - project: Manual
      paths: ["{}"]
"#,
        db.display(),
        env.path("manual").display()
    ));

    ok(env.aven_config(["project", "create", "Commented"]));
    ok(env.aven_config([
        "project",
        "path",
        "add",
        "commented",
        mapped.to_str().unwrap(),
    ]));
    ok(env.aven_config([
        "project",
        "path",
        "remove",
        "commented",
        mapped.to_str().unwrap(),
    ]));

    let config = fs::read_to_string(env.config_file()).unwrap();
    contains_all(
        &config,
        &[
            "# top comment",
            "# db comment",
            "# project comment",
            "# manual override comment",
            "project: Manual",
        ],
    );
    contains_none(
        &config,
        &["commented", "# aven-managed project path mapping"],
    );
}

#[test]
fn project_override_infers_configured_project() {
    let env = TestEnv::new();
    let db = env.db("override.sqlite");
    let repo = env.path("agentic-task-manager");
    init_git_repo(&repo);
    env.write_config(&format!(
        r#"local:
  db_path: "{}"

project:
  overrides:
    - project: "Aven"
      paths: ["{}"]
"#,
        db.display(),
        repo.display()
    ));

    let output = ok(aven_config_in(&env, &repo, ["add", "override inference"]));
    contains_all(&output, &["project=aven"]);
}

#[test]
fn project_path_command_remaps_same_path_in_config() {
    let env = TestEnv::new();
    let db = env.db("mapping-before-override.sqlite");
    let repo = env.path("repo");
    init_git_repo(&repo);
    env.write_config(&format!(
        r#"local:
  db_path: "{}"

project:
  overrides:
    - project: "Override"
      paths: ["{}"]
"#,
        db.display(),
        repo.display()
    ));

    ok(env.aven_config(["project", "create", "Override"]));
    ok(env.aven_config([
        "project",
        "create",
        "Mapped",
        "--path",
        repo.to_str().unwrap(),
    ]));
    let output = ok(aven_config_in(&env, &repo, ["add", "mapping precedence"]));
    contains_all(&output, &["project=mapped"]);
    let config = fs::read_to_string(env.config_file()).unwrap();
    contains_all(&config, &["workspace: default", "project: mapped"]);
    contains_none(&config, &["project: Override"]);
}

#[test]
fn project_override_takes_precedence_over_git_root() {
    let env = TestEnv::new();
    let db = env.db("override-before-git.sqlite");
    let repo = env.path("repo-name");
    init_git_repo(&repo);
    env.write_config(&format!(
        r#"local:
  db_path: "{}"

project:
  overrides:
    - project: "Override"
      paths: ["{}"]
"#,
        db.display(),
        repo.display()
    ));

    let output = ok(aven_config_in(&env, &repo, ["add", "override precedence"]));
    contains_all(&output, &["project=override"]);
}

#[test]
fn longest_matching_project_override_wins() {
    let env = TestEnv::new();
    let db = env.db("longest-override.sqlite");
    let repo = env.path("repo");
    let nested = repo.join("nested");
    init_git_repo(&repo);
    fs::create_dir_all(&nested).unwrap();
    env.write_config(&format!(
        r#"local:
  db_path: "{}"

project:
  overrides:
    - project: "Outer"
      paths: ["{}"]
    - project: "Inner"
      paths: ["{}"]
"#,
        db.display(),
        repo.display(),
        nested.display()
    ));

    let output = ok(aven_config_in(&env, &nested, ["add", "nested override"]));
    contains_all(&output, &["project=inner"]);
}

#[test]
fn missing_project_override_paths_are_skipped() {
    let env = TestEnv::new();
    let db = env.db("missing-override.sqlite");
    let repo = env.path("git-fallback");
    init_git_repo(&repo);
    env.write_config(&format!(
        r#"local:
  db_path: "{}"

project:
  overrides:
    - project: "Missing"
      paths: ["{}"]
"#,
        db.display(),
        env.path("missing").display()
    ));

    let output = ok(aven_config_in(&env, &repo, ["add", "git fallback"]));
    contains_all(&output, &["project=git-fallback"]);
}

#[test]
fn requires_project_without_mapping_or_git() {
    let env = TestEnv::new();
    let db = env.db("none.sqlite");
    let cwd = env.path("no-project");
    fs::create_dir_all(&cwd).unwrap();
    let error = fail(env.aven_in(&db, &cwd, ["add", "no project"]));
    contains_all(&error, &["error project-required"]);
}

#[test]
fn ignores_inherited_git_environment_for_project_inference() {
    let env = TestEnv::new();
    let db = env.db("inherited-git.sqlite");
    let cwd = env.path("no-project");
    fs::create_dir_all(&cwd).unwrap();
    let work_tree = env.path("git-inferred");
    fs::create_dir_all(&work_tree).unwrap();
    let status = clean_git_command()
        .args(["init", "-q"])
        .current_dir(&work_tree)
        .status()
        .expect("git init");
    assert!(status.success(), "git init failed");
    let git_dir = work_tree.join(".git");
    let output = Command::new(common::bin())
        .arg("--db")
        .arg(&db)
        .args(["add", "no project"])
        .current_dir(&cwd)
        .env("GIT_DIR", &git_dir)
        .env("GIT_WORK_TREE", &work_tree)
        .output()
        .expect("run aven with inherited git env");

    let error = fail(output);
    contains_all(&error, &["error project-required"]);
}

fn aven_config_in<I, S>(env: &TestEnv, cwd: &std::path::Path, args: I) -> std::process::Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = command();
    command
        .env("XDG_STATE_HOME", env.state_dir())
        .env("AVEN_CONFIG_DIR", env.config_dir().join("aven"))
        .env_remove("AVEN_DB")
        .env_remove("AVEN_SYNC_SERVER")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("run aven with config in cwd")
}

fn init_git_repo(repo: &std::path::Path) {
    fs::create_dir_all(repo).unwrap();
    ok_git(clean_git_command().args(["init", "-q"]).current_dir(repo));
}

fn ok_git(command: &mut Command) {
    let output = command
        .env("GIT_AUTHOR_NAME", "Aven Tests")
        .env("GIT_AUTHOR_EMAIL", "tests@example.com")
        .env("GIT_COMMITTER_NAME", "Aven Tests")
        .env("GIT_COMMITTER_EMAIL", "tests@example.com")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn clean_git_command() -> Command {
    let mut command = Command::new("git");
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
    command
}

#[test]
fn reports_near_project_matches() {
    let env = TestEnv::new();
    let db = env.db("near-project.sqlite");
    ok(env.aven(&db, ["project", "create", "homelab"]));

    let error = fail(env.aven(&db, ["add", "near project", "--project", "home-lab"]));
    contains_all(
        &error,
        &[
            "error unknown-project",
            "choice homelab",
            "retry with an exact project",
        ],
    );
}

#[test]
fn creates_projects_that_only_share_a_short_suffix() {
    let env = TestEnv::new();
    let db = env.db("project-suffix.sqlite");
    ok(env.aven(&db, ["workspace", "create", "client"]));
    ok(env.aven(
        &db,
        [
            "--workspace",
            "client",
            "project",
            "create",
            "core-service-worker",
        ],
    ));

    let output = ok(env.aven(
        &db,
        [
            "--workspace",
            "client",
            "add",
            "regional billing worker task",
            "--project",
            "regional-billing-service-worker",
        ],
    ));
    contains_all(&output, &["project=regional-billing-service-worker"]);
}

#[test]
fn reports_unknown_label_choices() {
    let env = TestEnv::new();
    let db = env.db("near-label.sqlite");
    ok(env.aven(&db, ["label", "create", "bug"]));

    let error = fail(env.aven(
        &db,
        ["add", "bad label", "--project", "app", "--label", "bux"],
    ));
    contains_all(
        &error,
        &[
            "error unknown-label",
            "choice bug",
            "create the label explicitly",
        ],
    );
}

#[test]
fn rejects_invalid_status_and_priority() {
    let env = TestEnv::new();
    let db = env.db("invalid.sqlite");

    let error = fail(env.aven(
        &db,
        [
            "add",
            "bad priority",
            "--project",
            "app",
            "--priority",
            "now",
        ],
    ));
    contains_all(
        &error,
        &[
            "error invalid-priority",
            "choices=none,low,medium,high,urgent",
        ],
    );

    let task_ref =
        common::extract_ref(&ok(env.aven(&db, ["add", "valid task", "--project", "app"])));
    let error = fail(env.aven(&db, ["update", &task_ref, "--status", "blocked"]));
    contains_all(
        &error,
        &[
            "error invalid-status",
            "choices=inbox,backlog,todo,active,done,canceled",
        ],
    );
}
