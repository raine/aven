mod common;

use std::process::Command;

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
    env.write_config(&format!(
        r#"local:
  db_path: "{}"
"#,
        db.display()
    ));

    ok(env.aven_config(["workspace", "create", "alpha"]));
    ok(env.aven_config(["workspace", "create", "beta"]));
    ok(env.aven_config(["--workspace", "alpha", "project", "create", "app"]));
    ok(env.aven_config(["--workspace", "beta", "project", "create", "app"]));
    ok(env.aven_config([
        "--workspace",
        "alpha",
        "project",
        "path",
        "add",
        "app",
        mapped.to_str().expect("utf8 path"),
    ]));
    ok(env.aven_config([
        "--workspace",
        "beta",
        "project",
        "path",
        "add",
        "app",
        mapped.to_str().expect("utf8 path"),
    ]));

    ok(env.aven_config([
        "--workspace",
        "alpha",
        "project",
        "path",
        "remove",
        "app",
        mapped.to_str().expect("utf8 path"),
    ]));

    let beta_ref = extract_ref(&ok(aven_config_in(
        &env,
        &mapped,
        ["--workspace", "beta", "add", "beta inferred"],
    )));
    let beta = ok(env.aven_config(["--workspace", "beta", "show", &beta_ref, "--full"]));
    contains_all(&beta, &["project=app"]);

    let alpha_error = fail(aven_config_in(
        &env,
        &mapped,
        ["--workspace", "alpha", "add", "alpha inferred"],
    ));
    contains_all(&alpha_error, &["project-required"]);
}

fn aven_config_in<I, S>(env: &TestEnv, cwd: &std::path::Path, args: I) -> std::process::Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = Command::new(common::bin());
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

fn sqlite_scalar(db: &std::path::Path, sql: &str) -> String {
    let output = Command::new("sqlite3")
        .arg(db)
        .arg(sql)
        .output()
        .expect("read sqlite scalar");
    assert!(output.status.success(), "sqlite failed");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn workspace_id(db: &std::path::Path, key: &str) -> String {
    let sql = format!("SELECT id FROM workspaces WHERE key = '{key}'");
    sqlite_scalar(db, &sql)
}

fn latest_payload(db: &std::path::Path, op_type: &str) -> String {
    let sql = format!(
        "SELECT payload FROM changes WHERE op_type = '{op_type}' ORDER BY local_seq DESC LIMIT 1"
    );
    sqlite_scalar(db, &sql)
}

#[test]
fn project_create_path_writes_active_workspace_id_and_key_to_config() {
    let env = TestEnv::new();
    let db = env.db("path-create-config.sqlite");
    let mapped = env.path("mapped");
    std::fs::create_dir_all(&mapped).expect("create mapped dir");
    env.write_config(&format!("local:\n  db_path: \"{}\"\n", db.display()));

    ok(env.aven_config(["workspace", "create", "alpha"]));
    ok(env.aven_config([
        "--workspace",
        "alpha",
        "project",
        "create",
        "app",
        "--path",
        mapped.to_str().expect("utf8 path"),
    ]));

    let alpha_id = workspace_id(&db, "alpha");
    let config = std::fs::read_to_string(env.config_file()).expect("read config");
    contains_all(
        &config,
        &[
            "# aven-managed project path mapping",
            &format!("workspace_id: {alpha_id}"),
            "workspace: alpha",
            "project: app",
        ],
    );
}

#[test]
fn project_path_list_scopes_and_filters_by_active_workspace() {
    let env = TestEnv::new();
    let db = env.db("path-list.sqlite");
    let app_path = env.path("client app");
    let docs_path = env.path("docs");
    let beta_path = env.path("beta-app");
    std::fs::create_dir_all(&app_path).expect("create app dir");
    std::fs::create_dir_all(&docs_path).expect("create docs dir");
    std::fs::create_dir_all(&beta_path).expect("create beta dir");
    env.write_config(&format!(
        r#"local:
  db_path: "{}"
"#,
        db.display()
    ));

    ok(env.aven_config(["workspace", "create", "alpha"]));
    ok(env.aven_config(["workspace", "create", "beta"]));
    ok(env.aven_config(["--workspace", "alpha", "project", "create", "Client App"]));
    ok(env.aven_config(["--workspace", "alpha", "project", "create", "docs"]));
    ok(env.aven_config(["--workspace", "beta", "project", "create", "app"]));
    ok(env.aven_config([
        "--workspace",
        "alpha",
        "project",
        "path",
        "add",
        "client-app",
        app_path.to_str().expect("utf8 app path"),
    ]));
    ok(env.aven_config([
        "--workspace",
        "alpha",
        "project",
        "path",
        "add",
        "docs",
        docs_path.to_str().expect("utf8 docs path"),
    ]));
    ok(env.aven_config([
        "--workspace",
        "beta",
        "project",
        "path",
        "add",
        "app",
        beta_path.to_str().expect("utf8 beta path"),
    ]));

    let app_path = std::fs::canonicalize(app_path).expect("canonical app path");
    let docs_path = std::fs::canonicalize(docs_path).expect("canonical docs path");
    let beta_path = std::fs::canonicalize(beta_path).expect("canonical beta path");

    let all = ok(env.aven_config(["--workspace", "alpha", "project", "path", "list"]));
    let expected_all = format!(
        "client-app path={}\ndocs path={}\n",
        serde_json::to_string(app_path.to_str().expect("utf8 app path")).expect("quote app path"),
        serde_json::to_string(docs_path.to_str().expect("utf8 docs path"))
            .expect("quote docs path")
    );
    assert_eq!(all, expected_all);
    contains_none(&all, &[beta_path.to_str().expect("utf8 beta path")]);

    let app = ok(env.aven_config([
        "--workspace",
        "alpha",
        "project",
        "path",
        "list",
        "Client App",
    ]));
    let expected_app = format!(
        "client-app path={}\n",
        serde_json::to_string(app_path.to_str().expect("utf8 app path")).expect("quote app path")
    );
    assert_eq!(app, expected_app);

    let docs = ok(env.aven_config(["--workspace", "alpha", "project", "path", "list", "docs"]));
    contains_all(&docs, &[docs_path.to_str().expect("utf8 docs path")]);
    contains_none(&docs, &[app_path.to_str().expect("utf8 app path")]);

    let missing =
        fail(env.aven_config(["--workspace", "alpha", "project", "path", "list", "missing"]));
    contains_all(&missing, &["error unknown-project input=missing"]);

    ok(env.aven_config(["--workspace", "alpha", "project", "create", "empty"]));
    let no_paths =
        ok(env.aven_config(["--workspace", "alpha", "project", "path", "list", "empty"]));
    assert!(no_paths.is_empty(), "expected no output\n{no_paths}");
}

#[test]
fn active_workspace_payloads_pair_id_and_key_for_writes() {
    let env = TestEnv::new();
    let db = env.db("active-payloads.sqlite");
    ok(env.aven(&db, ["workspace", "create", "client"]));
    let task_ref = extract_ref(&ok(env.aven(
        &db,
        [
            "--workspace",
            "client",
            "add",
            "client task",
            "--project",
            "app",
        ],
    )));
    let client_id = workspace_id(&db, "client");

    ok(env.aven(
        &db,
        [
            "--workspace",
            "client",
            "update",
            &task_ref,
            "--title",
            "renamed",
        ],
    ));
    let set_field = latest_payload(&db, "set_field");
    contains_all(
        &set_field,
        &[
            &format!("\"workspace_id\":\"{client_id}\""),
            "\"workspace_key\":\"client\"",
        ],
    );

    ok(env.aven(
        &db,
        ["--workspace", "client", "note", &task_ref, "scoped note"],
    ));
    let note_add = latest_payload(&db, "note_add");
    contains_all(
        &note_add,
        &[
            &format!("\"workspace_id\":\"{client_id}\""),
            "\"workspace_key\":\"client\"",
        ],
    );

    let sql = format!(
        "INSERT INTO conflicts(workspace_id, task_id, field, base_version, local_value, remote_value,
         local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
         SELECT '{client_id}', id, 'priority', NULL, 'none', 'urgent', NULL, 'REMOTECHANGE0001', 'local', 'remote', 't', 0
         FROM tasks WHERE workspace_id = '{client_id}'"
    );
    let output = Command::new("sqlite3")
        .arg(&db)
        .arg(sql)
        .output()
        .expect("seed conflict");
    assert!(output.status.success(), "sqlite failed");
    ok(env.aven(
        &db,
        [
            "--workspace",
            "client",
            "conflict",
            "resolve",
            &task_ref,
            "priority",
            "--value",
            "urgent",
        ],
    ));
    let resolve_field = latest_payload(&db, "resolve_field");
    contains_all(
        &resolve_field,
        &[
            &format!("\"workspace_id\":\"{client_id}\""),
            "\"workspace_key\":\"client\"",
        ],
    );
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
        INSERT INTO projects(id, workspace_id, key, name, prefix, created_at, updated_at)
        SELECT 'PROJECT' || key || '000000', id, 'app', 'app', 'APP', 't', 't' FROM workspaces WHERE key IN ('alpha', 'beta');
        INSERT INTO tasks(workspace_id, id, title, description, project_id, status, priority, created_at, updated_at)
        SELECT w.id, 'ABCD000000000000', 'alpha task', '', p.id, 'inbox', 'none', 't', 't' FROM workspaces w JOIN projects p ON p.workspace_id = w.id WHERE w.key = 'alpha';
        INSERT INTO tasks(workspace_id, id, title, description, project_id, status, priority, created_at, updated_at)
        SELECT w.id, 'ABCDE00000000000', 'beta task', '', p.id, 'inbox', 'none', 't', 't' FROM workspaces w JOIN projects p ON p.workspace_id = w.id WHERE w.key = 'beta';
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
