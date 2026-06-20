mod common;

use common::{TestEnv, TestServer, command, contains_all, contains_none, extract_ref, fail, ok};

fn sync(env: &TestEnv, db: &std::path::Path, server: &TestServer) {
    let output = ok(env.atm(db, ["sync", "--server", &server.url]));
    contains_all(&output, &["synced", "cursor="]);
}

#[test]
fn workspace_commands_manage_names_and_ambiguity() {
    let env = TestEnv::new();
    let db = env.db("workspaces.sqlite");

    let initial = ok(env.atm(&db, ["workspace", "list"]));
    contains_all(&initial, &["default", "name=\"default\""]);

    let created = ok(env.atm(&db, ["workspace", "create", "Client Work"]));
    contains_all(&created, &["created-workspace", "client-work", "name=\"Client Work\""]);

    let renamed = ok(env.atm(
        &db,
        ["workspace", "rename", "client-work", "Consulting"],
    ));
    contains_all(&renamed, &["renamed-workspace", "consulting", "name=\"Consulting\""]);

    let ambiguous = fail(env.atm(&db, ["list"]));
    contains_all(&ambiguous, &["workspace-required", "--workspace"]);

    ok(env.atm(&db, ["--workspace", "default", "list"]));
    ok(env.atm(&db, ["--workspace", "consulting", "list"]));
}

#[test]
fn workspace_scoped_commands_keep_data_isolated() {
    let env = TestEnv::new();
    let db = env.db("scoped.sqlite");

    ok(env.atm(&db, ["workspace", "create", "alpha"]));
    ok(env.atm(&db, ["workspace", "create", "beta"]));
    ok(env.atm(&db, ["--workspace", "alpha", "label", "create", "bug"]));
    ok(env.atm(&db, ["--workspace", "beta", "label", "create", "bug"]));

    let alpha_ref = extract_ref(&ok(env.atm(
        &db,
        [
            "--workspace",
            "alpha",
            "add",
            "alpha task",
            "--project",
            "app",
            "--label",
            "bug",
        ],
    )));
    let beta_ref = extract_ref(&ok(env.atm(
        &db,
        [
            "--workspace",
            "beta",
            "add",
            "beta task",
            "--project",
            "app",
            "--label",
            "bug",
        ],
    )));

    let alpha = ok(env.atm(&db, ["--workspace", "alpha", "list"]));
    contains_all(&alpha, &[&alpha_ref, "alpha task", "labels=bug"]);
    contains_none(&alpha, &[&beta_ref, "beta task"]);

    let beta = ok(env.atm(&db, ["--workspace", "beta", "list"]));
    contains_all(&beta, &[&beta_ref, "beta task", "labels=bug"]);
    contains_none(&beta, &[&alpha_ref, "alpha task"]);
}

#[test]
fn workspace_config_default_and_routes_select_active_workspace() {
    let env = TestEnv::new();
    let db = env.db("routes.sqlite");
    let alpha_dir = env.path("alpha-dir");
    let beta_dir = env.path("beta-dir");
    std::fs::create_dir_all(&alpha_dir).expect("create alpha dir");
    std::fs::create_dir_all(&beta_dir).expect("create beta dir");

    ok(env.atm(&db, ["workspace", "create", "alpha"]));
    ok(env.atm(&db, ["workspace", "create", "beta"]));
    env.write_config(&format!(
        r#"[local]
db_path = "{}"

[workspace]
default = "beta"

[[workspace.routes]]
workspace = "alpha"
paths = ["{}"]
"#,
        db.display(),
        alpha_dir.display()
    ));

    ok(env.atm_config(["label", "create", "bug"]));
    ok(env.atm_config([
        "add",
        "default beta task",
        "--project",
        "app",
        "--label",
        "bug",
    ]));

    ok(env.atm_config([
        "--workspace",
        "alpha",
        "label",
        "create",
        "bug",
    ]));
    ok(env.atm_config([
        "--workspace",
        "alpha",
        "add",
        "routed alpha task",
        "--project",
        "app",
        "--label",
        "bug",
    ]));

    let beta = ok(env.atm_config(["list"]));
    contains_all(&beta, &["default beta task"]);
    contains_none(&beta, &["routed alpha task"]);

    let alpha = ok(command()
        .env(
            "ATM_CONFIG_DIR",
            env.config_dir().join("agentic-task-manager"),
        )
        .env_remove("ATM_DB")
        .env_remove("ATM_SYNC_SERVER")
        .current_dir(&alpha_dir)
        .args(["list"])
        .output()
        .expect("run atm in routed cwd"));
    contains_all(&alpha, &["routed alpha task"]);
    contains_none(&alpha, &["default beta task"]);
}

#[test]
fn sync_converges_workspace_records_and_scoped_tasks() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");

    ok(env.atm(&a, ["workspace", "create", "client"]));
    ok(env.atm(&a, ["--workspace", "client", "label", "create", "bug"]));
    let task_ref = extract_ref(&ok(env.atm(
        &a,
        [
            "--workspace",
            "client",
            "add",
            "client task",
            "--project",
            "app",
            "--label",
            "bug",
        ],
    )));

    sync(&env, &a, &server);
    sync(&env, &b, &server);

    let workspaces = ok(env.atm(&b, ["workspace", "list"]));
    contains_all(&workspaces, &["client", "name=\"client\""]);

    let client = ok(env.atm(&b, ["--workspace", "client", "list"]));
    contains_all(&client, &[&task_ref, "client task", "labels=bug"]);

    let default = ok(env.atm(&b, ["--workspace", "default", "list"]));
    contains_none(&default, &["client task"]);
}
