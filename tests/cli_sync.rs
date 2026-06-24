mod common;

use std::time::Duration;

use common::{
    TestEnv, TestProcess, TestServer, contains_all, contains_none, extract_ref, fail, meta_value,
    ok, scalar_i64,
};
use serde_json::{Value, json};

fn sync(env: &TestEnv, db: &std::path::Path, server: &TestServer) {
    let output = ok(env.aven(db, ["sync", "--server", &server.url]));
    contains_all(&output, &["synced", "cursor="]);
}

fn wire_change(op_type: &str, entity_type: &str, entity_id: &str, payload: Value) -> Value {
    json!({
        "change_id": "0123456789ABCDEF",
        "client_id": "client-a",
        "local_seq": 1,
        "entity_type": entity_type,
        "entity_id": entity_id,
        "field": null,
        "op_type": op_type,
        "payload": payload,
        "base_version": null,
        "created_at": "2026-01-01T00:00:00Z",
        "server_seq": null,
    })
}

fn task_change(op_type: &str, payload: Value) -> Value {
    wire_change(op_type, "task", "0123456789ABCDE0", payload)
}

async fn post_sync(server: &TestServer, change: Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{}/sync", server.url))
        .json(&json!({
            "protocol_version": 1,
            "client_id": "client-a",
            "after": 0,
            "changes": [change],
        }))
        .send()
        .await
        .expect("post sync")
}

async fn assert_server_log_empty(server: &TestServer) {
    let response = reqwest::Client::new()
        .post(format!("{}/sync", server.url))
        .json(&json!({
            "protocol_version": 1,
            "client_id": "audit-client",
            "after": 0,
            "changes": [],
        }))
        .send()
        .await
        .expect("pull server log");

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.expect("sync response json");
    assert_eq!(body["changes"].as_array().expect("changes array").len(), 0);
}

fn assert_task_field_versions(db: &std::path::Path) {
    assert_eq!(scalar_i64(db, "SELECT count(*) FROM field_versions"), 6);
    assert_eq!(
        scalar_i64(
            db,
            "SELECT count(*) FROM field_versions
             WHERE field IN ('title','description','project','status','priority','deleted')",
        ),
        6
    );
}

async fn rejected_sync(server: &TestServer, change: Value, expected: &str) {
    let response = post_sync(server, change).await;
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body = response.text().await.expect("error body");
    contains_all(&body, &["error invalid-sync-change", expected]);
    assert_server_log_empty(server).await;
}

fn project_change_json(change_id: &str, key: &str) -> serde_json::Value {
    serde_json::json!({
        "change_id": change_id,
        "client_id": "old-client",
        "local_seq": 1,
        "entity_type": "project",
        "entity_id": key,
        "field": null,
        "op_type": "create_project",
        "payload": {
            "key": key,
            "name": "Legacy",
            "prefix": "LEG",
            "created_at": "2026-01-01T00:00:00Z"
        },
        "base_version": null,
        "created_at": "2026-01-01T00:00:00Z",
        "server_seq": null
    })
}

fn assert_sync_protocol_rejected(
    env: &TestEnv,
    server: &TestServer,
    body: &str,
    expected_error: &str,
) {
    let (status, text) = post_sync_json(&server.url, body);

    assert_eq!(status, 400);
    contains_all(&text, &[expected_error]);
    assert_eq!(
        scalar_i64(&env.path("server.sqlite"), "SELECT count(*) FROM changes"),
        0
    );
}

fn post_sync_json(url: &str, body: &str) -> (u16, String) {
    use std::io::{Read as _, Write as _};
    use std::net::TcpStream;

    let host = url.strip_prefix("http://").expect("loopback http url");
    let mut stream = TcpStream::connect(host).expect("connect sync server");
    write!(
        stream,
        "POST /sync HTTP/1.1\r\n\
         Host: {host}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    )
    .expect("write sync request");

    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read sync response");
    let (head, body) = raw.split_once("\r\n\r\n").expect("http response split");
    let status = head
        .lines()
        .next()
        .expect("status line")
        .split_whitespace()
        .nth(1)
        .expect("status code")
        .parse::<u16>()
        .expect("numeric status");
    (status, body.to_string())
}

#[test]
fn offline_creates_converge() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    assert!(
        server.url.starts_with("http://127.0.0.1:"),
        "unexpected server url: {}",
        server.url
    );
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");

    let a_ref = extract_ref(&ok(
        env.aven(&a, ["add", "offline from a", "--project", "app"])
    ));
    let b_ref = extract_ref(&ok(
        env.aven(&b, ["add", "offline from b", "--project", "app"])
    ));

    sync(&env, &a, &server);
    sync(&env, &b, &server);
    sync(&env, &a, &server);

    let list_a = ok(env.aven(&a, ["list", "--all"]));
    let list_b = ok(env.aven(&b, ["list", "--all"]));
    contains_all(
        &list_a,
        &[&a_ref, &b_ref, "offline from a", "offline from b"],
    );
    contains_all(
        &list_b,
        &[&a_ref, &b_ref, "offline from a", "offline from b"],
    );
}

#[test]
fn independent_field_edits_converge() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");

    let task_ref = extract_ref(&ok(
        env.aven(&a, ["add", "merge fields", "--project", "app"])
    ));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    ok(env.aven(&a, ["update", &task_ref, "--status", "active"]));
    ok(env.aven(&b, ["update", &task_ref, "--priority", "high"]));
    sync(&env, &a, &server);
    sync(&env, &b, &server);
    sync(&env, &a, &server);

    let merged = ok(env.aven(&a, ["show", &task_ref]));
    contains_all(&merged, &["status=active", "priority=high"]);
    contains_none(&merged, &["conflicts=yes"]);
}

#[test]
fn notes_and_labels_converge() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");

    for label in ["docs", "sync", "bug"] {
        ok(env.aven(&a, ["label", "create", label]));
    }
    let task_ref = extract_ref(&ok(
        env.aven(&a, ["add", "merge notes and labels", "--project", "app"])
    ));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    ok(env.aven_stdin(&a, ["note", &task_ref, "--stdin"], "note a\n"));
    ok(env.aven_stdin(&b, ["note", &task_ref, "--stdin"], "note b\n"));
    ok(env.aven(&a, ["update", &task_ref, "--label", "docs"]));
    ok(env.aven(&b, ["update", &task_ref, "--label", "sync"]));
    sync(&env, &a, &server);
    sync(&env, &b, &server);
    sync(&env, &a, &server);

    let full = ok(env.aven(&a, ["show", &task_ref, "--full"]));
    contains_all(&full, &["note a", "note b", "labels=docs,sync"]);

    ok(env.aven(&a, ["update", &task_ref, "--remove-label", "docs"]));
    ok(env.aven(&b, ["update", &task_ref, "--label", "bug"]));
    sync(&env, &a, &server);
    sync(&env, &b, &server);
    sync(&env, &a, &server);

    let labels = ok(env.aven(&a, ["show", &task_ref]));
    contains_all(&labels, &["labels=bug,sync"]);
    contains_none(&labels, &["labels=docs"]);
}

#[test]
fn soft_delete_syncs_and_restores() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");

    let task_ref = extract_ref(&ok(
        env.aven(&a, ["add", "temporary task", "--project", "app"])
    ));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    ok(env.aven(&a, ["delete", &task_ref]));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    let normal_b = ok(env.aven(&b, ["list"]));
    contains_none(&normal_b, &[&task_ref, "temporary task"]);
    let all_b = ok(env.aven(&b, ["list", "--all"]));
    contains_all(&all_b, &[&task_ref, "temporary task", "deleted=yes"]);

    ok(env.aven(&a, ["restore", &task_ref]));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    let restored_b = ok(env.aven(&b, ["list"]));
    contains_all(&restored_b, &[&task_ref, "temporary task"]);
    contains_none(&restored_b, &["deleted=yes"]);
}

#[test]
fn sync_auth_config_init_includes_placeholder() {
    let env = TestEnv::new();
    ok(env.aven_config(["config", "init"]));

    let text = std::fs::read_to_string(env.config_file()).expect("read config");
    contains_all(&text, &["sync:", "auth_token: ''"]);
}

#[test]
fn sync_auth_missing_token_is_rejected() {
    let server_env = TestEnv::new();
    server_env.write_config(
        r#"
sync:
  auth_token: "secret"
"#,
    );
    let server = TestServer::start_configured(&server_env, "server.sqlite");

    let client_env = TestEnv::new();
    let client = client_env.db("client.sqlite");
    ok(client_env.aven(&client, ["add", "auth missing", "--project", "app"]));

    let error = fail(client_env.aven(&client, ["sync", "--server", &server.url]));
    contains_all(&error, &["401"]);
    assert_eq!(
        scalar_i64(
            &server_env.path("server.sqlite"),
            "SELECT count(*) FROM changes"
        ),
        0
    );
}

#[test]
fn sync_auth_wrong_token_is_rejected() {
    let server_env = TestEnv::new();
    server_env.write_config(
        r#"
sync:
  auth_token: "secret"
"#,
    );
    let server = TestServer::start_configured(&server_env, "server.sqlite");

    let client_env = TestEnv::new();
    let client = client_env.db("client.sqlite");
    client_env.write_config(&format!(
        r#"
local:
  db_path: "{}"

sync:
  server_url: "{}"
  auth_token: "wrong"
"#,
        client.display(),
        server.url
    ));
    ok(client_env.aven_config(["add", "auth wrong", "--project", "app"]));

    let error = fail(client_env.aven_config(["sync"]));
    contains_all(&error, &["401"]);
    assert_eq!(
        scalar_i64(
            &server_env.path("server.sqlite"),
            "SELECT count(*) FROM changes"
        ),
        0
    );
}

#[test]
fn sync_auth_correct_token_syncs() {
    let server_env = TestEnv::new();
    server_env.write_config(
        r#"
sync:
  auth_token: "secret"
"#,
    );
    let server = TestServer::start_configured(&server_env, "server.sqlite");

    let client_env = TestEnv::new();
    let a = client_env.db("client-a.sqlite");
    let b = client_env.db("client-b.sqlite");
    client_env.write_config(&format!(
        r#"
sync:
  server_url: "{}"
  auth_token: "secret"
"#,
        server.url
    ));

    let task_ref = extract_ref(&ok(
        client_env.aven(&a, ["add", "auth synced", "--project", "app"])
    ));
    ok(client_env.aven_config([
        "--db",
        a.to_str().expect("utf8 db path"),
        "sync",
        "--server",
        &server.url,
    ]));
    ok(client_env.aven_config([
        "--db",
        b.to_str().expect("utf8 db path"),
        "sync",
        "--server",
        &server.url,
    ]));

    let shown = ok(client_env.aven(&b, ["show", &task_ref]));
    contains_all(&shown, &[&task_ref, "auth synced"]);
}

#[test]
fn sync_auth_loopback_without_token_still_syncs() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");

    let task_ref = extract_ref(&ok(
        env.aven(&a, ["add", "local no auth", "--project", "app"])
    ));
    ok(env.aven(&a, ["sync", "--server", &server.url]));
    ok(env.aven(&b, ["sync", "--server", &server.url]));

    let shown = ok(env.aven(&b, ["show", &task_ref]));
    contains_all(&shown, &[&task_ref, "local no auth"]);
}

#[test]
fn sync_auth_public_bind_requires_token_even_with_unsafe_flag() {
    let env = TestEnv::new();
    let server_db = env.path("server.sqlite");
    let error = fail(env.aven_config([
        "server",
        "--bind",
        "0.0.0.0:0",
        "--unsafe-public-bind",
        "--data",
        server_db.to_str().expect("utf8 db path"),
    ]));

    contains_all(
        &error,
        &[
            "error sync-auth-token-required",
            "set sync.auth_token in config.yaml",
        ],
    );
}

#[test]
fn sync_server_bind_startup_output_includes_scope() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);

    contains_all(&server.output(), &["listening url=", "scope=loopback"]);
    assert!(server.url.starts_with("http://127.0.0.1:"));
}

#[test]
fn sync_server_bind_private_requires_token() {
    let env = TestEnv::new();
    let error = fail(env.aven_config([
        "server",
        "--bind",
        "100.64.0.1:0",
        "--data",
        env.path("server.sqlite").to_str().expect("utf8 temp path"),
    ]));

    contains_all(
        &error,
        &[
            "error private-bind-requires-auth",
            "set sync.auth_token or bind to 127.0.0.1",
        ],
    );
}

#[test]
fn sync_server_bind_public_stays_behind_unsafe_flag() {
    let env = TestEnv::new();
    let error = fail(env.aven_config([
        "server",
        "--bind",
        "0.0.0.0:0",
        "--data",
        env.path("server.sqlite").to_str().expect("utf8 temp path"),
    ]));

    contains_all(&error, &["error public-bind-requires --unsafe-public-bind"]);
}

#[test]
fn sync_server_bind_public_warns_when_enabled() {
    let env = TestEnv::new();
    env.write_config(
        r#"
sync:
  auth_token: "secret"
"#,
    );
    let process = TestProcess::start_server(
        &env,
        [
            "--bind",
            "0.0.0.0:0",
            "--unsafe-public-bind",
            "--data",
            env.path("server.sqlite").to_str().expect("utf8 temp path"),
        ],
    );

    process.wait_for_log("warning public bind enabled", Duration::from_secs(5));
    process.wait_for_log("scope=public", Duration::from_secs(5));
    let output = process.output();
    assert!(
        output.find("warning public bind enabled").unwrap()
            < output.find("listening url=").unwrap(),
        "warning should be printed before listening line\n{output}"
    );
}

#[tokio::test]
async fn sync_server_rejects_unknown_operations_and_entity_types() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);

    rejected_sync(
        &server,
        task_change("move_task", json!({})),
        "op_type=move_task",
    )
    .await;
    rejected_sync(
        &server,
        wire_change(
            "create_task",
            "project",
            "0123456789ABCDE0",
            json!({ "title": "bad", "project_key": "app" }),
        ),
        "entity_type=project",
    )
    .await;
}

#[tokio::test]
async fn sync_server_rejects_invalid_field_names() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let mut change = task_change("set_field", json!({ "value": "x" }));
    change
        .as_object_mut()
        .expect("change object")
        .insert("field".to_string(), json!("labels"));

    rejected_sync(&server, change, "field=labels").await;
}

#[tokio::test]
async fn sync_server_rejects_invalid_status_priority_and_deleted_values() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);

    let mut bad_status = task_change("set_field", json!({ "value": "blocked" }));
    bad_status
        .as_object_mut()
        .expect("change object")
        .insert("field".to_string(), json!("status"));
    rejected_sync(&server, bad_status, "invalid-status").await;

    let bad_priority = task_change(
        "create_task",
        json!({
            "title": "bad priority",
            "project_key": "app",
            "priority": "soon",
        }),
    );
    rejected_sync(&server, bad_priority, "invalid-priority").await;

    let bad_create_status = task_change(
        "create_task",
        json!({
            "title": "bad status",
            "project_key": "app",
            "status": "blocked",
        }),
    );
    rejected_sync(&server, bad_create_status, "invalid-status").await;

    let mut bad_deleted = task_change("set_field", json!({ "value": "true" }));
    bad_deleted
        .as_object_mut()
        .expect("change object")
        .insert("field".to_string(), json!("deleted"));
    rejected_sync(&server, bad_deleted, "deleted").await;
}

#[tokio::test]
async fn sync_server_rejects_malformed_ids() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);

    let mut bad_change_id = task_change("label_add", json!({ "label": "sync" }));
    bad_change_id
        .as_object_mut()
        .expect("change object")
        .insert("change_id".to_string(), json!("short"));
    rejected_sync(&server, bad_change_id, "change_id").await;

    let bad_task_id = wire_change(
        "label_add",
        "task",
        "not-a-task-id",
        json!({ "label": "sync" }),
    );
    rejected_sync(&server, bad_task_id, "entity_id").await;

    let bad_note_id = task_change(
        "note_add",
        json!({
            "note_id": "not-a-note-id",
            "body": "body",
            "created_at": "2026-01-01T00:00:00Z",
        }),
    );
    rejected_sync(&server, bad_note_id, "note_id").await;
}

#[tokio::test]
async fn sync_server_rejects_client_supplied_server_sequence() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let mut change = task_change("label_add", json!({ "label": "sync" }));
    change
        .as_object_mut()
        .expect("change object")
        .insert("server_seq".to_string(), json!(99));

    rejected_sync(&server, change, "server_seq").await;
}

#[test]
fn valid_offline_batch_with_related_operations_still_syncs() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");

    ok(env.aven(&a, ["label", "create", "offline"]));
    let task_ref = extract_ref(&ok(
        env.aven(&a, ["add", "offline batch", "--project", "app"])
    ));
    ok(env.aven(&a, ["update", &task_ref, "--label", "offline"]));
    ok(env.aven_stdin(&a, ["note", &task_ref, "--stdin"], "batch note\n"));

    sync(&env, &a, &server);
    sync(&env, &b, &server);

    let full = ok(env.aven(&b, ["show", &task_ref, "--full"]));
    contains_all(&full, &["offline batch", "labels=offline", "batch note"]);
}

#[test]
fn task_create_seeds_versioned_field_versions_locally_and_remotely() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");

    ok(env.aven(&a, ["add", "version seed", "--project", "app"]));
    assert_task_field_versions(&a);

    sync(&env, &a, &server);
    sync(&env, &b, &server);
    assert_task_field_versions(&b);
}

#[test]
fn current_protocol_version_sync_succeeds() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");

    let task_ref = extract_ref(&ok(
        env.aven(&a, ["add", "versioned sync", "--project", "app"])
    ));

    sync(&env, &a, &server);
    sync(&env, &b, &server);

    let shown = ok(env.aven(&b, ["show", &task_ref]));
    contains_all(&shown, &[&task_ref, "versioned sync"]);
}

#[test]
fn missing_request_protocol_version_is_rejected_before_changes_are_stored() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let body = serde_json::json!({
        "client_id": "old-client",
        "after": 0,
        "changes": [project_change_json("missing-version-change", "missing-version")]
    })
    .to_string();

    assert_sync_protocol_rejected(
        &env,
        &server,
        &body,
        "error sync-protocol-unsupported client=0 server=1",
    );
}

#[test]
fn old_request_protocol_version_is_rejected_before_changes_are_stored() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let body = serde_json::json!({
        "protocol_version": 0,
        "client_id": "old-client",
        "after": 0,
        "changes": [project_change_json("old-version-change", "old-version")]
    })
    .to_string();

    assert_sync_protocol_rejected(
        &env,
        &server,
        &body,
        "error sync-protocol-unsupported client=0 server=1",
    );
}

#[test]
fn newer_request_protocol_version_is_rejected_before_changes_are_stored() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let body = serde_json::json!({
        "protocol_version": 2,
        "client_id": "new-client",
        "after": 0,
        "changes": [project_change_json("new-version-change", "new-version")]
    })
    .to_string();

    assert_sync_protocol_rejected(
        &env,
        &server,
        &body,
        "error sync-protocol-unsupported client=2 server=1",
    );
}

#[test]
fn wrong_response_protocol_version_is_rejected() {
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::net::TcpListener;
    use std::thread;

    let env = TestEnv::new();
    let db = env.db("client.sqlite");
    ok(env.aven(&db, ["add", "seed", "--project", "app"]));
    let changes_before = scalar_i64(&db, "SELECT count(*) FROM changes");
    let sync_cursor_before = meta_value(&db, "sync_cursor");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake sync server");
    let url = format!("http://{}", listener.local_addr().expect("fake sync addr"));
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept sync request");
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
        let mut line = String::new();
        loop {
            line.clear();
            reader.read_line(&mut line).expect("read request line");
            if line == "\r\n" || line.is_empty() {
                break;
            }
        }
        let body = serde_json::json!({
            "protocol_version": 0,
            "cursor": 7,
            "changes": [project_change_json("remote-change", "rogue")]
        })
        .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("write fake response");
    });

    let error = fail(env.aven(&db, ["sync", "--server", &url]));
    contains_all(
        &error,
        &["error sync-protocol-unsupported client=1 server=0"],
    );
    assert_eq!(
        scalar_i64(&db, "SELECT count(*) FROM changes"),
        changes_before
    );
    assert_eq!(
        scalar_i64(&db, "SELECT count(*) FROM projects WHERE key = 'rogue'"),
        0
    );
    assert_eq!(meta_value(&db, "sync_cursor"), sync_cursor_before);
    server.join().expect("fake sync server exits");
}
