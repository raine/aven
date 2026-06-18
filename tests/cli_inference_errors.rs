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

    ok(env.atm(
        &db,
        [
            "project",
            "create",
            "mapped",
            "--path",
            mapped.to_str().unwrap(),
        ],
    ));
    let output = ok(env.atm_in(&db, &nested, ["add", "mapped inference"]));
    contains_all(&output, &["project=mapped"]);
}

#[test]
fn infers_project_from_git_root() {
    let env = TestEnv::new();
    let db = env.db("git.sqlite");
    let repo = env.path("git-inferred");
    fs::create_dir_all(&repo).unwrap();
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .expect("git init");
    assert!(status.success(), "git init failed");

    let output = ok(env.atm_in(&db, &repo, ["add", "git inference"]));
    contains_all(&output, &["project=git-inferred", "created G"]);
}

#[test]
fn requires_project_without_mapping_or_git() {
    let env = TestEnv::new();
    let db = env.db("none.sqlite");
    let cwd = env.path("no-project");
    fs::create_dir_all(&cwd).unwrap();
    let error = fail(env.atm_in(&db, &cwd, ["add", "no project"]));
    contains_all(&error, &["error project-required"]);
}

#[test]
fn reports_near_project_matches() {
    let env = TestEnv::new();
    let db = env.db("near-project.sqlite");
    ok(env.atm(&db, ["project", "create", "homelab"]));

    let error = fail(env.atm(&db, ["add", "near project", "--project", "home-lab"]));
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
    ok(env.atm(&db, ["label", "create", "bug"]));

    let error = fail(env.atm(
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

    let error = fail(env.atm(
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
        common::extract_ref(&ok(env.atm(&db, ["add", "valid task", "--project", "app"])));
    let error = fail(env.atm(&db, ["update", &task_ref, "--status", "blocked"]));
    contains_all(
        &error,
        &[
            "error invalid-status",
            "choices=inbox,backlog,todo,active,done,canceled",
        ],
    );
}
