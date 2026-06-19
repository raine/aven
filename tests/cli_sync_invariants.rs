mod common;

use std::process::Command;

use common::{
    TestEnv, TestServer, contains_all, contains_none, extract_ref, fail, meta_value, ok, scalar_i64,
};

fn sync(env: &TestEnv, db: &std::path::Path, server: &TestServer) -> String {
    ok(env.atm(db, ["sync", "--server", &server.url]))
}

#[test]
fn sync_server_url_is_pinned_and_normalized() {
    let env = TestEnv::new();
    let server_a = TestServer::start_with_data(&env, "server-a.sqlite");
    let server_b = TestServer::start_with_data(&env, "server-b.sqlite");
    let db = env.db("pinned.sqlite");

    ok(env.atm(&db, ["sync", "--server", &format!("{}/", server_a.url)]));
    assert_eq!(
        meta_value(&db, "sync_server_url"),
        Some(server_a.url.clone())
    );

    ok(env.atm(&db, ["sync", "--server", &server_a.url]));
    let error = fail(env.atm(&db, ["sync", "--server", &server_b.url]));
    contains_all(
        &error,
        &["error sync-server-changed", "use a fresh database"],
    );
}

#[test]
fn repeated_sync_is_idempotent_and_acknowledges_changes() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");

    ok(env.atm(&a, ["label", "create", "sync"]));
    let task_ref = extract_ref(&ok(env.atm(
        &a,
        [
            "add",
            "idempotent sync",
            "--project",
            "app",
            "--label",
            "sync",
        ],
    )));
    ok(env.atm_stdin(&a, ["note", &task_ref, "--stdin"], "only once\n"));

    let first = sync(&env, &a, &server);
    contains_all(&first, &["pushed=4", "pulled=0", "cursor="]);
    assert_eq!(
        scalar_i64(&a, "SELECT count(*) FROM changes WHERE server_seq IS NULL"),
        0
    );
    let first_cursor = meta_value(&a, "sync_cursor").expect("first cursor");

    sync(&env, &b, &server);
    let repeat_b = sync(&env, &b, &server);
    contains_all(&repeat_b, &["pushed=0", "pulled=0"]);
    assert_eq!(scalar_i64(&b, "SELECT count(*) FROM tasks"), 1);
    assert_eq!(scalar_i64(&b, "SELECT count(*) FROM notes"), 1);
    assert_eq!(scalar_i64(&b, "SELECT count(*) FROM task_labels"), 1);

    let repeat_a = sync(&env, &a, &server);
    contains_all(&repeat_a, &["pushed=0", "pulled=0"]);
    assert_eq!(meta_value(&a, "sync_cursor"), Some(first_cursor));
}

#[test]
fn config_env_and_flag_precedence_for_sync_server() {
    let env = TestEnv::new();
    let server_config = TestServer::start_with_data(&env, "server-config.sqlite");
    let server_env = TestServer::start_with_data(&env, "server-env.sqlite");
    let server_flag = TestServer::start_with_data(&env, "server-flag.sqlite");
    let db = env.db("precedence.sqlite");
    env.write_config(&format!(
        r#"
[local]
db_path = "{}"

[sync]
enabled = true
server_url = "{}"
"#,
        db.display(),
        server_config.url
    ));

    ok(env.atm_config(["sync"]));
    assert_eq!(
        meta_value(&db, "sync_server_url"),
        Some(server_config.url.clone())
    );

    let env_db = env.db("env.sqlite");
    let env_output = Command::new(common::bin())
        .env(
            "ATM_CONFIG_DIR",
            env.config_dir().join("agentic-task-manager"),
        )
        .env("ATM_DB", &env_db)
        .env("ATM_SYNC_SERVER", &server_env.url)
        .arg("sync")
        .output()
        .expect("run atm with env server");
    ok(env_output);
    assert_eq!(
        meta_value(&env_db, "sync_server_url"),
        Some(server_env.url.clone())
    );

    let flag_db = env.db("flag.sqlite");
    ok(env.atm_config([
        "--db",
        flag_db.to_str().expect("utf8 db path"),
        "sync",
        "--server",
        &server_flag.url,
    ]));
    assert_eq!(
        meta_value(&flag_db, "sync_server_url"),
        Some(server_flag.url.clone())
    );
}

#[test]
fn db_flag_bypasses_config_except_for_sync_settings() {
    let server_env = TestEnv::new();
    server_env.write_config(
        r#"
[sync]
auth_token = "secret"
"#,
    );
    let server = TestServer::start_configured(&server_env, "server.sqlite");

    let env = TestEnv::new();
    let config_db = env.db("config.sqlite");
    let flag_db = env.db("flag.sqlite");
    env.write_config(&format!(
        r#"
[local]
db_path = "{}"

[sync]
enabled = true
server_url = "{}"
auth_token = "secret"
"#,
        config_db.display(),
        server.url
    ));

    let task_ref = extract_ref(&ok(env.atm_config([
        "--db",
        flag_db.to_str().expect("utf8 db path"),
        "add",
        "flag task",
        "--project",
        "app",
    ])));

    let config_list = ok(env.atm_config(["list", "--all"]));
    contains_none(&config_list, &["flag task"]);

    let flag_list = ok(env.atm(&flag_db, ["list", "--all"]));
    contains_all(&flag_list, &[&task_ref, "flag task"]);

    let sync = ok(env.atm_config(["--db", flag_db.to_str().expect("utf8 db path"), "sync"]));
    contains_all(&sync, &["synced", "pushed="]);
    assert_eq!(
        meta_value(&flag_db, "sync_server_url"),
        Some(server.url.clone())
    );
}
