mod common;

use common::{TestEnv, TestServer, command, contains_all, contains_none, extract_ref, fail, ok};

fn sync(env: &TestEnv, db: &std::path::Path, server: &TestServer) {
    let output = ok(env.aven(db, ["sync", "--server", &server.url]));
    contains_all(&output, &["synced", "cursor="]);
}

#[test]
fn workspace_commands_manage_names_and_ambiguity() {
    let env = TestEnv::new();
    let db = env.db("workspaces.sqlite");

    let initial = ok(env.aven(&db, ["workspace", "list"]));
    contains_all(&initial, &["default", "name=\"default\""]);

    let created = ok(env.aven(&db, ["workspace", "create", "Client Work"]));
    contains_all(
        &created,
        &["created-workspace", "client-work", "name=\"Client Work\""],
    );

    let renamed = ok(env.aven(&db, ["workspace", "rename", "client-work", "Consulting"]));
    contains_all(
        &renamed,
        &["renamed-workspace", "consulting", "name=\"Consulting\""],
    );

    ok(env.aven(&db, ["list"]));
    ok(env.aven(&db, ["--workspace", "default", "list"]));
    ok(env.aven(&db, ["--workspace", "consulting", "list"]));
}

#[test]
fn workspace_scoped_commands_keep_data_isolated() {
    let env = TestEnv::new();
    let db = env.db("scoped.sqlite");

    ok(env.aven(&db, ["workspace", "create", "alpha"]));
    ok(env.aven(&db, ["workspace", "create", "beta"]));
    ok(env.aven(&db, ["--workspace", "alpha", "label", "create", "bug"]));
    ok(env.aven(&db, ["--workspace", "beta", "label", "create", "bug"]));

    let alpha_ref = extract_ref(&ok(env.aven(
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
    let beta_ref = extract_ref(&ok(env.aven(
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

    let alpha = ok(env.aven(&db, ["--workspace", "alpha", "list"]));
    contains_all(&alpha, &[&alpha_ref, "alpha task", "labels=bug"]);
    contains_none(&alpha, &[&beta_ref, "beta task"]);

    let beta = ok(env.aven(&db, ["--workspace", "beta", "list"]));
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

    ok(env.aven(&db, ["workspace", "create", "alpha"]));
    ok(env.aven(&db, ["workspace", "create", "beta"]));
    env.write_config(&format!(
        r#"local:
  db_path: "{}"

workspace:
  default: "beta"
  routes:
    - workspace: "alpha"
      paths: ["{}"]
"#,
        db.display(),
        alpha_dir.display()
    ));

    ok(env.aven_config(["label", "create", "bug"]));
    ok(env.aven_config([
        "add",
        "default beta task",
        "--project",
        "app",
        "--label",
        "bug",
    ]));

    ok(env.aven_config(["--workspace", "alpha", "label", "create", "bug"]));
    ok(env.aven_config([
        "--workspace",
        "alpha",
        "add",
        "routed alpha task",
        "--project",
        "app",
        "--label",
        "bug",
    ]));

    let beta = ok(env.aven_config(["list"]));
    contains_all(&beta, &["default beta task"]);
    contains_none(&beta, &["routed alpha task"]);

    let alpha = ok(command()
        .env("AVEN_CONFIG_DIR", env.config_dir().join("aven"))
        .env_remove("AVEN_DB")
        .env_remove("AVEN_SYNC_SERVER")
        .current_dir(&alpha_dir)
        .args(["list"])
        .output()
        .expect("run aven in routed cwd"));
    contains_all(&alpha, &["routed alpha task"]);
    contains_none(&alpha, &["default beta task"]);
}

#[test]
fn project_path_remove_only_affects_active_workspace() {
    let env = TestEnv::new();
    let db = env.db("paths.sqlite");
    let mapped = env.path("mapped");
    std::fs::create_dir_all(&mapped).expect("create mapped dir");

    ok(env.aven(&db, ["workspace", "create", "alpha"]));
    ok(env.aven(&db, ["workspace", "create", "beta"]));
    ok(env.aven(&db, ["--workspace", "alpha", "project", "create", "app"]));
    ok(env.aven(&db, ["--workspace", "beta", "project", "create", "app"]));
    ok(env.aven(
        &db,
        [
            "--workspace",
            "alpha",
            "project",
            "path",
            "add",
            "app",
            mapped.to_str().expect("utf8 path"),
        ],
    ));
    ok(env.aven(
        &db,
        [
            "--workspace",
            "beta",
            "project",
            "path",
            "add",
            "app",
            mapped.to_str().expect("utf8 path"),
        ],
    ));

    ok(env.aven(
        &db,
        [
            "--workspace",
            "alpha",
            "project",
            "path",
            "remove",
            "app",
            mapped.to_str().expect("utf8 path"),
        ],
    ));

    let beta_ref = extract_ref(&ok(env.aven_in(
        &db,
        &mapped,
        ["--workspace", "beta", "add", "beta inferred"],
    )));
    let beta = ok(env.aven(&db, ["--workspace", "beta", "show", &beta_ref, "--full"]));
    contains_all(&beta, &["project=app"]);

    let alpha_error = fail(env.aven_in(
        &db,
        &mapped,
        ["--workspace", "alpha", "add", "alpha inferred"],
    ));
    contains_all(&alpha_error, &["project-required"]);
}

#[test]
fn display_suffix_ignores_other_workspaces() {
    let env = TestEnv::new();
    let db = env.db("suffixes.sqlite");

    ok(env.aven(&db, ["workspace", "create", "alpha"]));
    ok(env.aven(&db, ["workspace", "create", "beta"]));
    let alpha_id = "ABCD000000000000";
    let beta_id = "ABCDE00000000000";
    let sql = "
        INSERT INTO projects(workspace_id, key, name, prefix, created_at, updated_at)
        SELECT id, 'app', 'app', 'APP', 't', 't' FROM workspaces WHERE key IN ('alpha', 'beta');
        INSERT INTO tasks(workspace_id, id, title, description, project_key, status, priority, created_at, updated_at)
        SELECT id, 'ABCD000000000000', 'alpha task', '', 'app', 'inbox', 'none', 't', 't' FROM workspaces WHERE key = 'alpha';
        INSERT INTO tasks(workspace_id, id, title, description, project_key, status, priority, created_at, updated_at)
        SELECT id, 'ABCDE00000000000', 'beta task', '', 'app', 'inbox', 'none', 't', 't' FROM workspaces WHERE key = 'beta';
        INSERT INTO conflicts(workspace_id, task_id, field, local_value, remote_value, remote_change_id, variant_a, variant_b, created_at)
        SELECT id, 'ABCD000000000000', 'title', 'local', 'remote', 'REMOTECHANGE0000', 'a', 'b', 't' FROM workspaces WHERE key = 'alpha';
    ";
    let output = std::process::Command::new("sqlite3")
        .arg(&db)
        .arg(sql)
        .output()
        .expect("seed suffix data");
    assert!(output.status.success(), "sqlite failed");

    let conflicts = ok(env.aven(&db, ["--workspace", "alpha", "conflict", "list"]));
    contains_all(&conflicts, &["APP-ABCD", "alpha task"]);
    contains_none(&conflicts, &["APP-ABCD0", beta_id, alpha_id]);
}

#[test]
fn renamed_default_workspace_still_opens_database() {
    let env = TestEnv::new();
    let db = env.db("renamed-default.sqlite");

    ok(env.aven(&db, ["workspace", "rename", "default", "personal"]));
    let workspaces = ok(env.aven(&db, ["workspace", "list"]));
    contains_all(&workspaces, &["personal", "name=\"personal\""]);
}

#[test]
fn sync_rejects_field_updates_for_missing_tasks() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let client = env.db("client.sqlite");
    ok(env.aven(&client, ["workspace", "create", "client"]));

    let workspace_id = {
        let output = std::process::Command::new("sqlite3")
            .arg(&client)
            .arg("SELECT id FROM workspaces WHERE key = 'client'")
            .output()
            .expect("read workspace id");
        assert!(output.status.success(), "sqlite failed");
        String::from_utf8(output.stdout)
            .expect("utf8 workspace id")
            .trim()
            .to_string()
    };
    let server_db = env.path("server.sqlite");
    ok(env.aven(&server_db, ["workspace", "list"]));
    let sql = format!(
        "INSERT OR IGNORE INTO workspaces(id, key, name, created_at, updated_at) VALUES ('{workspace_id}', 'client', 'client', 't', 't');\
         INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field, op_type, payload, base_version, created_at, server_seq)\
         VALUES ('REMOTECHANGE0002', 'remote', 1, 'task', '0123456789ABCDE0', 'title', 'set_field', json_object('workspace_id', '{workspace_id}', 'workspace_key', 'client', 'value', 'ghost'), NULL, 't', 1);"
    );
    let output = std::process::Command::new("sqlite3")
        .arg(&server_db)
        .arg(sql)
        .output()
        .expect("seed remote change");
    assert!(output.status.success(), "sqlite failed");

    let error = fail(env.aven(&client, ["sync", "--server", &server.url]));
    contains_all(&error, &["task-not-found"]);
}

#[test]
fn sync_converges_workspace_records_and_scoped_tasks() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");

    ok(env.aven(&a, ["workspace", "create", "client"]));
    ok(env.aven(&a, ["--workspace", "client", "label", "create", "bug"]));
    let task_ref = extract_ref(&ok(env.aven(
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

    let workspaces = ok(env.aven(&b, ["workspace", "list"]));
    contains_all(&workspaces, &["client", "name=\"client\""]);

    let client = ok(env.aven(&b, ["--workspace", "client", "list"]));
    contains_all(&client, &[&task_ref, "client task", "labels=bug"]);

    let default = ok(env.aven(&b, ["--workspace", "default", "list"]));
    contains_none(&default, &["client task"]);
}
