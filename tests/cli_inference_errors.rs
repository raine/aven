mod common;

use std::fs;
use std::process::Command;

use common::{TestEnv, contains_all, fail, ok};

#[test]
fn infers_project_from_path_mapping() {
    let env = TestEnv::new();
    let db = env.db("mapping.sqlite");
    let mapped = env.path("mapped");
    let nested = mapped.join("sub");
    fs::create_dir_all(&nested).unwrap();

    ok(env.aven(
        &db,
        [
            "project",
            "create",
            "mapped",
            "--path",
            mapped.to_str().unwrap(),
        ],
    ));
    let output = ok(env.aven_in(&db, &nested, ["add", "mapped inference"]));
    contains_all(&output, &["project=mapped"]);
}

#[test]
fn infers_project_from_git_root() {
    let env = TestEnv::new();
    let db = env.db("git.sqlite");
    let repo = env.path("git-inferred");
    fs::create_dir_all(&repo).unwrap();
    let status = clean_git_command()
        .args(["init", "-q"])
        .current_dir(&repo)
        .env_remove("GIT_DIR")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_PREFIX")
        .status()
        .expect("git init");
    assert!(status.success(), "git init failed");

    let output = ok(env.aven_in(&db, &repo, ["add", "git inference"]));
    contains_all(&output, &["project=git-inferred", "created G"]);
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
