mod common;

use common::{TestEnv, contains_all, contains_none, extract_ref, fail, ok};
use std::path::Path;

fn projects(output: std::process::Output, expected: &[&str], note: Option<&str>) {
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let rows: serde_json::Value = serde_json::from_str(&ok(output)).unwrap();
    let rows = rows.as_array().expect("bare JSON array");
    let actual: Vec<_> = rows
        .iter()
        .map(|row| row["project"].as_str().unwrap())
        .collect();
    assert_eq!(actual.len(), expected.len(), "{rows:?}");
    for project in expected {
        assert!(actual.contains(project), "{actual:?}");
    }
    if let Some(source) = note {
        assert_eq!(stderr.lines().count(), 1, "{stderr}");
        contains_all(
            &stderr,
            &[
                &format!("scope: project {}", expected[0]),
                source,
                "--all-projects",
            ],
        );
    } else {
        assert!(stderr.is_empty(), "{stderr}");
    }
}

fn seed(env: &TestEnv, db: &Path) {
    for project in ["app", "other"] {
        ok(env.aven(db, ["add", "needle", "--project", project]));
    }
}

#[test]
fn browsing_defaults_to_git_project_with_explicit_and_config_overrides() {
    let env = TestEnv::new();
    let db = env.db("tasks.sqlite");
    seed(&env, &db);
    let mapped = env.path("app");
    std::fs::create_dir_all(mapped.join(".git")).unwrap();
    let unmapped = env.path("unmapped");
    std::fs::create_dir_all(&unmapped).unwrap();
    for command in [vec!["list"], vec!["search", "needle"]] {
        let run = |cwd: &Path, flags: &[&str]| {
            env.aven_in(
                &db,
                cwd,
                [command.clone(), flags.to_vec(), vec!["--json"]].concat(),
            )
        };
        projects(run(&mapped, &[]), &["app"], Some("git root name"));
        projects(run(&mapped, &["-A"]), &["app", "other"], None);
        projects(run(&mapped, &["-p", "other"]), &["other"], None);
        projects(run(&mapped, &["-p"]), &["app"], None);
        projects(run(&unmapped, &[]), &["app", "other"], None);
        env.write_config("scope: workspace\n");
        projects(run(&mapped, &[]), &["app", "other"], None);
        projects(run(&mapped, &["-p"]), &["app"], None);
        projects(run(&mapped, &["-p", "app"]), &["app"], None);
        env.write_config("scope: project\n");
    }
    env.write_config("scope: invalid\n");
    contains_all(&fail(env.aven(&db, ["list"])), &["could not parse"]);
}

#[tokio::test]
async fn config_path_and_database_path_inference_report_the_winning_source() {
    let env = TestEnv::new();
    let db = env.db("tasks.sqlite");
    seed(&env, &db);
    let mapped = env.path("other");
    std::fs::create_dir_all(mapped.join("nested")).unwrap();
    let mapped = std::fs::canonicalize(mapped).unwrap();
    std::fs::create_dir_all(mapped.join(".git")).unwrap();
    // Legacy database mappings and config overrides share the inference order.
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", db.display()))
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_paths(workspace_id, project_id, path) SELECT workspace_id, id, ? FROM projects WHERE key='app'")
        .bind(mapped.to_str().unwrap()).execute(&pool).await.unwrap();
    pool.close().await;
    projects(
        env.aven_in(&db, &mapped, ["list", "--json"]),
        &["app"],
        Some("project path mapping"),
    );
    env.write_config(&format!(
        "project:\n  overrides:\n    - project: other\n      paths: ['{}']\n",
        mapped.display()
    ));
    projects(
        env.aven_in(&db, &mapped.join("nested"), ["search", "needle", "--json"]),
        &["other"],
        Some("config override"),
    );
    env.write_config(&format!(
        "project:\n  overrides:\n    - project: missing\n      paths: ['{}']\n",
        mapped.display()
    ));
    projects(
        env.aven_in(&db, &mapped, ["list", "--json"]),
        &["app"],
        Some("project path mapping"),
    );
}

#[test]
fn every_list_mode_is_scoped_and_refs_remain_workspace_wide() {
    let env = TestEnv::new();
    let db = env.db("tasks.sqlite");
    let mapped = env.path("app");
    std::fs::create_dir_all(mapped.join(".git")).unwrap();
    for project in ["app", "other"] {
        for (title, flags) in [
            ("ready", vec!["--status", "todo"]),
            ("deferred", vec!["--available-at", "tomorrow"]),
            ("late", vec!["--due", "2000-01-01"]),
            ("epic", vec!["--epic"]),
            ("finished", vec!["--status", "done"]),
            ("deleted", vec![]),
        ] {
            let task = extract_ref(&ok(env.aven(
                &db,
                [vec!["add", title, "--project", project], flags].concat(),
            )));
            if title == "deleted" {
                ok(env.aven(&db, ["delete", &task]));
            }
        }
        let blocked = extract_ref(&ok(env.aven(&db, ["add", "blocked", "--project", project])));
        let blocker = extract_ref(&ok(env.aven(&db, ["add", "blocker", "--project", project])));
        ok(env.aven(&db, ["dep", "add", &blocked, &blocker]));
        if project == "other" {
            contains_all(
                &ok(env.aven_in(&db, &mapped, ["show", &blocked])),
                &["blocked"],
            );
        }
    }
    for filter in [
        "--ready",
        "--blocked",
        "--overdue",
        "--upcoming",
        "--deleted",
        "--epics",
        "--all",
        "--open",
    ] {
        let scoped: serde_json::Value =
            serde_json::from_str(&ok(env.aven_in(&db, &mapped, ["list", filter, "--json"])))
                .unwrap();
        let all: serde_json::Value = serde_json::from_str(&ok(env.aven_in(
            &db,
            &mapped,
            ["list", filter, "--json", "-A"],
        )))
        .unwrap();
        let rows = scoped.as_array().unwrap();
        assert!(!rows.is_empty(), "{filter}");
        assert!(
            rows.iter().all(|row| row["project"] == "app"),
            "{filter}: {rows:?}"
        );
        assert_eq!(all.as_array().unwrap().len(), rows.len() * 2, "{filter}");
    }
    let output = env.aven_in(&db, &mapped, ["list", "--ready"]);
    contains_none(&ok(output), &["scope:"]);
}

#[test]
fn inference_uses_the_selected_workspace_and_root_flags_are_not_ignored() {
    let env = TestEnv::new();
    let db = env.db("tasks.sqlite");
    seed(&env, &db);
    let mapped = env.path("app");
    std::fs::create_dir_all(mapped.join(".git")).unwrap();
    ok(env.aven(&db, ["workspace", "create", "work"]));
    ok(env.aven(
        &db,
        ["--workspace", "work", "add", "needle", "--project", "other"],
    ));
    projects(
        env.aven_in(
            &db,
            &mapped,
            ["--workspace", "work", "search", "needle", "--json"],
        ),
        &["other"],
        None,
    );
    ok(env.aven(
        &db,
        ["--workspace", "work", "add", "needle", "--project", "app"],
    ));
    projects(
        env.aven_in(
            &db,
            &mapped,
            ["--workspace", "work", "search", "needle", "--json"],
        ),
        &["app"],
        Some("git root name"),
    );
    contains_all(
        &fail(env.aven(&db, ["-A", "list"])),
        &["place --project or --all-projects after the subcommand"],
    );
}
