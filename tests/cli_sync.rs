mod common;

use std::time::{Duration, Instant};

use common::{
    TestEnv, TestProcess, TestServer, contains_all, contains_none, extract_ref, fail, meta_value,
    ok, png_bytes, scalar_i64,
};
use serde_json::{Value, json};

const DEFAULT_WORKSPACE_ID: &str = "0000000000000000";
const REMOTE_PROJECT_ID: &str = "0000000000000001";
const SYNC_TASK_A_ID: &str = "AAAAAAAAAAAAAAAA";
const SYNC_TASK_B_ID: &str = "BBBBBBBBBBBBBBBB";
const SYNC_DEP_CHANGE_ID: &str = "CCCCCCCCCCCCCCCC";
const SYNC_TASK_A_CHANGE_ID: &str = "DDDDDDDDDDDDDDDD";
const SYNC_TASK_B_CHANGE_ID: &str = "EEEEEEEEEEEEEEEE";
const SYNC_OPPOSITE_DEP_CHANGE_ID: &str = "FFFFFFFFFFFFFFFF";
const SYNC_CLIENT_ID: &str = "GGGGGGGGGGGGGGGG";
const MAX_PUSH_BATCH: usize = 256;
const MAX_PULL_BATCH: usize = 512;
const SYNC_PROTOCOL_VERSION: u32 = 9;

fn sync_round_values(output: &str, key: &str) -> Vec<u64> {
    output
        .lines()
        .filter(|line| line.starts_with("synced "))
        .filter_map(|line| {
            line.split_whitespace()
                .find_map(|field| field.strip_prefix(&format!("{key}=")))
                .and_then(|value| value.parse().ok())
        })
        .collect()
}

fn sync(env: &TestEnv, db: &std::path::Path, server: &TestServer) {
    let output = ok(env.aven(db, ["sync", "--server", &server.url]));
    contains_all(&output, &["synced", "cursor="]);
}

fn exec_sql(db: &std::path::Path, sql: &str) {
    let output = std::process::Command::new("sqlite3")
        .arg(db)
        .arg(sql)
        .output()
        .expect("run sqlite");
    assert!(output.status.success(), "sqlite failed");
}

fn query_sql_scalar(db: &std::path::Path, sql: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = std::process::Command::new("sqlite3")
            .arg(db)
            .arg(sql)
            .output()
            .expect("run sqlite");
        if output.status.success() {
            return String::from_utf8_lossy(&output.stdout).trim().to_string();
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let busy =
            stderr.contains("database is locked") || stderr.contains("database table is locked");
        if !busy || Instant::now() >= deadline {
            panic!(
                "sqlite failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                stderr
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }
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

fn dependency_change(
    op_type: &str,
    change_id: &str,
    entity_id: &str,
    depends_on_task_id: &str,
) -> Value {
    dependency_change_with_payload(
        op_type,
        change_id,
        entity_id,
        json!({
            "workspace_id": "0000000000000000",
            "workspace_key": "default",
            "depends_on_task_id": depends_on_task_id,
        }),
    )
}

fn dependency_change_with_payload(
    op_type: &str,
    change_id: &str,
    entity_id: &str,
    payload: Value,
) -> Value {
    json!({
        "change_id": change_id,
        "client_id": "client-a",
        "local_seq": 1,
        "entity_type": "task",
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

fn create_task_payload(title: &str, project_key: &str, extra: Value) -> Value {
    let mut payload = json!({
        "title": title,
        "project_id": "0123456789ABCDE1",
        "project_key": project_key,
        "project_name": project_key,
        "project_prefix": "APP",
    });
    let object = payload.as_object_mut().expect("payload object");
    let extra = extra.as_object().expect("extra payload object");
    for (key, value) in extra {
        object.insert(key.clone(), value.clone());
    }
    payload
}

async fn post_sync(server: &TestServer, change: Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{}/sync", server.url))
        .json(&json!({
            "protocol_version": SYNC_PROTOCOL_VERSION,
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
            "protocol_version": SYNC_PROTOCOL_VERSION,
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

fn pending_push_acks(db: &std::path::Path, first_server_seq: i64) -> Vec<Value> {
    query_sql_scalar(
        db,
        "SELECT group_concat(change_id, ',') FROM changes WHERE server_seq IS NULL ORDER BY local_seq",
    )
    .split(',')
    .filter(|change_id| !change_id.is_empty())
    .enumerate()
    .map(|(index, change_id)| {
        json!({
            "change_id": change_id,
            "server_seq": first_server_seq + index as i64,
        })
    })
    .collect()
}

fn seed_task_with_pending_push_acks(
    env: &TestEnv,
    db_name: &str,
    title: &str,
) -> (std::path::PathBuf, Vec<Value>) {
    let db = env.db(db_name);
    ok(env.aven(&db, ["add", title, "--project", "app"]));
    let push_acks = pending_push_acks(&db, 99);
    (db, push_acks)
}

fn assert_task_field_versions(db: &std::path::Path) {
    assert_eq!(scalar_i64(db, "SELECT count(*) FROM field_versions"), 9);
    assert_eq!(
        scalar_i64(
            db,
            "SELECT count(*) FROM field_versions
             WHERE field IN ('title','description','project','status','priority','available_at','deleted','is_epic','due_on')",
        ),
        9
    );
}

async fn rejected_sync(server: &TestServer, change: Value, expected: &str) {
    let response = post_sync(server, change).await;
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body = response.text().await.expect("error body");
    contains_all(&body, &["error invalid-sync-change", expected]);
    assert_server_log_empty(server).await;
}

fn valid_create_project_change(change_id: &str, local_seq: i64) -> serde_json::Value {
    serde_json::json!({
        "change_id": change_id,
        "client_id": "client-a",
        "local_seq": local_seq,
        "entity_type": "project",
        "entity_id": format!("AAAAAAAAAAAA{:04}", local_seq),
        "field": null,
        "op_type": "create_project",
        "payload": {
            "key": format!("project-{local_seq}"),
            "name": format!("Project {local_seq}"),
            "prefix": format!("P{local_seq}"),
            "created_at": "2026-01-01T00:00:00Z"
        },
        "base_version": null,
        "created_at": "2026-01-01T00:00:00Z",
        "server_seq": null
    })
}

fn sync_pull_request(client_id: &str, after: usize) -> Value {
    json!({
        "protocol_version": SYNC_PROTOCOL_VERSION,
        "client_id": client_id,
        "after": after,
        "pull_limit": MAX_PULL_BATCH,
        "changes": [],
    })
}

fn note_delete_change(payload: Value) -> Value {
    let mut change = task_change("note_delete", payload);
    change
        .as_object_mut()
        .expect("change object")
        .insert("field".to_string(), json!("notes"));
    change
}

fn attachment_change_payload(extra: Value) -> Value {
    let mut payload = json!({
        "workspace_id": DEFAULT_WORKSPACE_ID,
        "workspace_key": "default",
        "attachment_id": "7KQ9A1X4MV2P8D6R",
        "sha256": "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        "byte_size": 12,
        "media_type": "image/png",
        "filename": "photo.png",
        "alt_text": "photo",
        "width": 320,
        "height": 240,
        "created_at": "2026-01-01T00:00:00Z"
    });
    let object = payload.as_object_mut().expect("payload object");
    let extra = extra.as_object().expect("extra payload object");
    for (key, value) in extra {
        object.insert(key.clone(), value.clone());
    }
    payload
}

fn attachment_add_change(payload: Value) -> Value {
    let mut change = task_change("attachment_add", payload);
    change
        .as_object_mut()
        .expect("change object")
        .insert("field".to_string(), json!("attachments"));
    change
}

fn attachment_delete_change(attachment_id: &str, deleted_at: &str) -> Value {
    let mut change = task_change(
        "attachment_delete",
        json!({
            "workspace_id": DEFAULT_WORKSPACE_ID,
            "workspace_key": "default",
            "attachment_id": attachment_id,
            "deleted_at": deleted_at
        }),
    );
    change
        .as_object_mut()
        .expect("change object")
        .insert("field".to_string(), json!("attachments"));
    change
}

fn local_task_id(db: &std::path::Path, title: &str) -> String {
    query_sql_scalar(db, &format!("SELECT id FROM tasks WHERE title = '{title}'"))
}

fn remote_attachment_add(change_id: &str, task_id: &str, server_seq: i64) -> Value {
    let mut change = attachment_add_change(attachment_change_payload(json!({})));
    change["change_id"] = json!(change_id);
    change["entity_id"] = json!(task_id);
    change["server_seq"] = json!(server_seq);
    change
}

fn remote_attachment_delete(
    change_id: &str,
    task_id: &str,
    attachment_id: &str,
    deleted_at: &str,
    server_seq: i64,
) -> Value {
    let mut change = attachment_delete_change(attachment_id, deleted_at);
    change["change_id"] = json!(change_id);
    change["entity_id"] = json!(task_id);
    change["server_seq"] = json!(server_seq);
    change
}

fn insert_attachment_row(db: &std::path::Path, task_id: &str, sha256: &str, deleted: bool) {
    let deleted_value = if deleted { 1 } else { 0 };
    let deleted_at = if deleted {
        "'2026-01-01T00:00:01Z'"
    } else {
        "NULL"
    };
    exec_sql(
        db,
        &format!(
            "INSERT INTO task_attachments(workspace_id, attachment_id, task_id, sha256, byte_size, media_type, filename, alt_text, width, height, created_at, created_by_change_id, deleted, deleted_at)
             VALUES ('{DEFAULT_WORKSPACE_ID}', '7KQ9A1X4MV2P8D6R', '{task_id}', '{sha256}', 12, 'image/png', 'photo.png', 'photo', 320, 240, '2026-01-01T00:00:00Z', 'LOCALCHANGE00001', {deleted_value}, {deleted_at})"
        ),
    );
}

fn seed_available_blob_for_hash(db: &std::path::Path, sha256: &str) {
    let mut blob_dir = db.as_os_str().to_os_string();
    blob_dir.push(".blobs");
    let blob_path = std::path::PathBuf::from(blob_dir)
        .join("objects")
        .join("sha256")
        .join(sha256);
    std::fs::create_dir_all(blob_path.parent().expect("blob parent")).expect("create blob dir");
    std::fs::write(&blob_path, b"placeholder").expect("write blob");
    exec_sql(
        db,
        &format!(
            "INSERT INTO blob_inventory(sha256, byte_size, media_type, available, first_seen_at, last_verified_at)
             VALUES ('{sha256}', 12, 'image/png', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
        ),
    );
}

fn seed_local_attachment_change(db: &std::path::Path, task_id: &str) {
    use sha2::{Digest, Sha256};

    let bytes = png_bytes(320, 240);
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let mut blob_dir = db.as_os_str().to_os_string();
    blob_dir.push(".blobs");
    let blob_path = std::path::PathBuf::from(blob_dir)
        .join("objects")
        .join("sha256")
        .join(&sha256);
    std::fs::create_dir_all(blob_path.parent().expect("blob parent")).expect("create blob dir");
    std::fs::write(&blob_path, &bytes).expect("write blob");
    let payload = json!({
        "workspace_id": DEFAULT_WORKSPACE_ID,
        "workspace_key": "default",
        "attachment_id": "7KQ9A1X4MV2P8D6R",
        "sha256": sha256,
        "byte_size": bytes.len(),
        "media_type": "image/png",
        "filename": "photo.png",
        "alt_text": "photo",
        "width": 320,
        "height": 240,
        "created_at": "2026-01-01T00:00:00Z"
    });
    exec_sql(
        db,
        &format!(
            "INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field, op_type, payload, base_version, created_at, server_seq)
             VALUES ('ATTACHMENTADD001', 'client-a', 100, 'task', '{task_id}', 'attachments', 'attachment_add', '{}', NULL, '2026-01-01T00:00:00Z', NULL)",
            payload.to_string().replace('\'', "''")
        ),
    );
    exec_sql(
        db,
        &format!(
            "INSERT INTO task_attachments(workspace_id, attachment_id, task_id, sha256, byte_size, media_type, filename, alt_text, width, height, created_at, created_by_change_id)
             VALUES ('{DEFAULT_WORKSPACE_ID}', '7KQ9A1X4MV2P8D6R', '{task_id}', '{sha256}', {}, 'image/png', 'photo.png', 'photo', 320, 240, '2026-01-01T00:00:00Z', 'ATTACHMENTADD001')",
            bytes.len()
        ),
    );
    exec_sql(
        db,
        &format!(
            "INSERT INTO blob_inventory(sha256, byte_size, media_type, available, first_seen_at, last_verified_at)
             VALUES ('{sha256}', {}, 'image/png', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            bytes.len()
        ),
    );
}

fn gzip_encode(data: &str) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data.as_bytes()).expect("write gzip body");
    encoder.finish().expect("finish gzip body")
}

fn gzip_decode(data: &[u8]) -> String {
    use std::io::Read;
    let mut decoder = flate2::read::GzDecoder::new(data);
    let mut result = String::new();
    decoder.read_to_string(&mut result).expect("decode gzip");
    result
}

fn post_sync_raw(host: &str, body: &[u8]) -> Vec<u8> {
    use std::io::{Read as _, Write as _};
    use std::net::TcpStream;
    let mut stream = TcpStream::connect(host).expect("connect sync server");
    write!(
        stream,
        "POST /sync HTTP/1.0\r\n\
         Content-Type: application/json\r\n\
         Content-Encoding: gzip\r\n\
         Accept-Encoding: gzip\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        body.len()
    )
    .expect("write sync request head");
    stream.write_all(body).expect("write sync request body");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read sync response");
    raw
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

fn remote_task_change(change_id: &str, task_id: &str, title: &str, server_seq: i64) -> Value {
    json!({
        "change_id": change_id,
        "client_id": SYNC_CLIENT_ID,
        "local_seq": server_seq,
        "entity_type": "task",
        "entity_id": task_id,
        "field": null,
        "op_type": "create_task",
        "payload": {
            "workspace_id": DEFAULT_WORKSPACE_ID,
            "workspace_key": "default",
            "title": title,
            "project_id": REMOTE_PROJECT_ID,
            "project_key": "app",
            "project_name": "app",
            "project_prefix": "APP"
        },
        "base_version": null,
        "created_at": format!("2026-01-01T00:00:{server_seq:02}Z"),
        "server_seq": server_seq,
    })
}

fn remote_dependency_change(
    change_id: &str,
    task_id: &str,
    depends_on_task_id: &str,
    server_seq: i64,
) -> Value {
    json!({
        "change_id": change_id,
        "client_id": SYNC_CLIENT_ID,
        "local_seq": server_seq,
        "entity_type": "task",
        "entity_id": task_id,
        "field": null,
        "op_type": "dependency_add",
        "payload": {
            "workspace_id": DEFAULT_WORKSPACE_ID,
            "workspace_key": "default",
            "depends_on_task_id": depends_on_task_id
        },
        "base_version": null,
        "created_at": format!("2026-01-01T00:00:{server_seq:02}Z"),
        "server_seq": server_seq,
    })
}

struct FakeSyncServer {
    url: String,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FakeSyncServer {
    fn url(&self) -> &str {
        &self.url
    }

    fn finish(mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let thread = self.thread.take().expect("fake sync server thread");
        thread.join().expect("fake sync server exits");
    }
}

impl Drop for FakeSyncServer {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct FakeSyncHttpResponse {
    status_line: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

fn start_fake_sync_http_server_sequence(responses: Vec<FakeSyncHttpResponse>) -> FakeSyncServer {
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::net::TcpListener;
    use std::thread;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake sync server");
    listener
        .set_nonblocking(true)
        .expect("set fake sync server nonblocking");
    let url = format!("http://{}", listener.local_addr().expect("fake sync addr"));
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let thread_stop = std::sync::Arc::clone(&stop);
    let thread = thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if thread_stop.load(std::sync::atomic::Ordering::Relaxed) {
                            return;
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept sync request: {error}"),
                }
            };
            stream
                .set_nonblocking(false)
                .expect("set fake sync stream blocking");
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut line = String::new();
            loop {
                line.clear();
                reader.read_line(&mut line).expect("read request line");
                if line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            write!(
                stream,
                "HTTP/1.1 {}\r\n\
                 content-type: {}\r\n\
                 content-length: {}\r\n\
                 connection: close\r\n\
                 \r\n",
                response.status_line,
                response.content_type,
                response.body.len(),
            )
            .expect("write fake response headers");
            stream
                .write_all(&response.body)
                .expect("write fake response body");
        }
    });
    FakeSyncServer {
        url,
        stop,
        thread: Some(thread),
    }
}

fn start_fake_sync_server_sequence(responses: Vec<Value>) -> FakeSyncServer {
    start_fake_sync_http_server_sequence(
        responses
            .into_iter()
            .map(|response| FakeSyncHttpResponse {
                status_line: "200 OK",
                content_type: "application/json",
                body: response.to_string().into_bytes(),
            })
            .collect(),
    )
}

fn start_fake_sync_server(response: Value) -> FakeSyncServer {
    start_fake_sync_server_sequence(vec![response])
}

fn start_authenticated_server(env: &TestEnv) -> TestServer {
    env.write_config(
        r#"
sync:
  auth_token: "secret"
"#,
    );
    TestServer::start_configured(env, "server.sqlite")
}

fn sync_response_with_push_acks(
    cursor: i64,
    push_acks: impl IntoIterator<Item = Value>,
    changes: impl IntoIterator<Item = Value>,
) -> Value {
    let changes = changes
        .into_iter()
        .enumerate()
        .map(|(index, mut change)| {
            if change["server_seq"].is_null() {
                change["server_seq"] = json!(index as i64 + 1);
            }
            change
        })
        .collect::<Vec<_>>();
    json!({
        "protocol_version": SYNC_PROTOCOL_VERSION,
        "cursor": cursor,
        "has_more": false,
        "push_acks": push_acks.into_iter().collect::<Vec<_>>(),
        "changes": changes,
    })
}

fn sync_response(cursor: i64, changes: impl IntoIterator<Item = Value>) -> Value {
    sync_response_with_push_acks(cursor, [], changes)
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
fn sync_http_error_includes_server_rejection_detail() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let db = env.db("invalid-change.sqlite");
    ok(env.aven(&db, ["add", "invalid push", "--project", "app"]));
    exec_sql(&db, "UPDATE changes SET op_type = 'invalid_test_op'");

    let error = fail(env.aven(&db, ["sync", "--server", &server.url]));

    contains_all(
        &error,
        &[
            "sync request failed with HTTP status 400",
            "error invalid-sync-change op_type=invalid_test_op",
        ],
    );
}

#[test]
fn sync_http_error_explains_protocol_upgrade_direction() {
    let env = TestEnv::new();
    let db = env.db("protocol-mismatch.sqlite");
    ok(env.aven(&db, ["add", "local task", "--project", "app"]));
    let server = start_fake_sync_http_server_sequence(
        [
            "error sync-protocol-unsupported client=9 server=5",
            "error sync-protocol-unsupported client=5 server=9",
        ]
        .into_iter()
        .map(|body| FakeSyncHttpResponse {
            status_line: "400 Bad Request",
            content_type: "text/plain; charset=utf-8",
            body: body.as_bytes().to_vec(),
        })
        .collect(),
    );

    let server_error = fail(env.aven(&db, ["sync", "--server", server.url()]));
    contains_all(
        &server_error,
        &[
            "client and server use incompatible sync protocol versions (client 9, server 5)",
            "upgrade the sync server",
        ],
    );
    contains_none(&server_error, &["sync-protocol-unsupported"]);

    let client_error = fail(env.aven(&db, ["sync", "--server", server.url()]));
    contains_all(
        &client_error,
        &[
            "client and server use incompatible sync protocol versions (client 5, server 9)",
            "upgrade the sync client",
        ],
    );
    contains_none(&client_error, &["sync-protocol-unsupported"]);
    server.finish();
}

#[test]
fn sync_http_error_uses_safe_fallback_for_undecodable_body() {
    let env = TestEnv::new();
    let db = env.db("undecodable-http-error.sqlite");
    ok(env.aven(&db, ["add", "local task", "--project", "app"]));
    let mut body = vec![0xff];
    body.extend_from_slice(b"error upstream-secret=do-not-print");
    let server = start_fake_sync_http_server_sequence(vec![FakeSyncHttpResponse {
        status_line: "400 Bad Request",
        content_type: "text/plain; charset=utf-8",
        body,
    }]);

    let error = fail(env.aven(&db, ["sync", "--server", server.url()]));

    contains_all(
        &error,
        &[
            "sync request failed with HTTP status 400",
            "server rejected the sync request",
            "check server logs",
            "verify client/server version compatibility",
        ],
    );
    contains_none(&error, &["upstream-secret", "do-not-print"]);
    server.finish();
}

#[test]
fn project_ids_survive_key_drift_across_sync() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");

    let seed_ref = extract_ref(&ok(env.aven(&a, ["add", "seed", "--project", "app"])));
    ok(env.aven(&a, ["project", "create", "Ops"]));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    exec_sql(
        &a,
        "UPDATE projects SET key = 'renamed-ops', name = 'Renamed Ops' WHERE key = 'ops'",
    );
    let drift_ref = extract_ref(&ok(
        env.aven(&a, ["add", "after drift", "--project", "renamed-ops"])
    ));
    ok(env.aven(&a, ["edit", &seed_ref, "--project", "renamed-ops"]));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    let drift = ok(env.aven(&b, ["show", &drift_ref]));
    contains_all(&drift, &["after drift", "OPS-"]);
    contains_none(&drift, &["renamed-ops"]);
    let seed = ok(env.aven(&b, ["show", &seed_ref]));
    contains_all(&seed, &["seed", "OPS-"]);
    contains_none(&seed, &["renamed-ops"]);
    assert_eq!(
        scalar_i64(
            &b,
            "SELECT count(*) FROM projects WHERE key = 'renamed-ops'"
        ),
        0
    );
}

#[test]
fn project_rename_syncs_by_project_id() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");

    let task_ref = extract_ref(&ok(
        env.aven(&a, ["add", "rename synced", "--project", "agent-offload"])
    ));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    ok(env.aven(
        &a,
        [
            "project",
            "rename",
            "agent-offload",
            "sideagent",
            "--prefix",
            "SIDE",
        ],
    ));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    let shown = ok(env.aven(&b, ["show", &task_ref]));
    contains_all(&shown, &["SIDE-", "rename synced"]);
    let projects = ok(env.aven(&b, ["project", "list"]));
    contains_all(&projects, &["sideagent prefix=SIDE"]);
    contains_none(&projects, &["agent-offload"]);
}

#[test]
fn remote_project_rename_updates_managed_path_mapping() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");
    let project_dir = env.path("client-b-project");
    std::fs::create_dir_all(&project_dir).unwrap();

    ok(env.aven(&a, ["add", "rename synced", "--project", "agent-offload"]));
    sync(&env, &a, &server);
    sync(&env, &b, &server);
    ok(env.aven(
        &b,
        [
            "project",
            "path",
            "add",
            "agent-offload",
            project_dir.to_str().unwrap(),
        ],
    ));

    ok(env.aven(
        &a,
        [
            "project",
            "rename",
            "agent-offload",
            "sideagent",
            "--prefix",
            "SIDE",
        ],
    ));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    let config = std::fs::read_to_string(env.config_file()).unwrap();
    contains_all(&config, &["project: sideagent"]);
    contains_none(&config, &["project: agent-offload"]);
}

#[test]
fn same_key_remote_project_writes_alias() {
    let env = TestEnv::new();
    let db = env.db("project-collision.sqlite");
    ok(env.aven(&db, ["add", "local seed", "--project", "app"]));
    exec_sql(&db, "UPDATE changes SET server_seq = local_seq + 90");
    let server = start_fake_sync_server(sync_response(
        1,
        [wire_change(
            "create_task",
            "task",
            "CCCCCCCCCCCCCCCC",
            json!({
                "title": "remote collision",
                "project_id": "BBBBBBBBBBBBBBBB",
                "project_key": "app",
                "project_name": "app",
                "project_prefix": "APP",
            }),
        )],
    ));

    ok(env.aven(&db, ["sync", "--server", server.url()]));
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT count(*) FROM tasks
             WHERE id = 'CCCCCCCCCCCCCCCC'
               AND project_id = (SELECT id FROM projects WHERE key = 'app')",
        ),
        1
    );
    assert_eq!(
        query_sql_scalar(
            &db,
            "SELECT source FROM tasks WHERE id = 'CCCCCCCCCCCCCCCC'",
        ),
        "unknown"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT count(*) FROM projects WHERE id = 'BBBBBBBBBBBBBBBB'",
        ),
        0
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT count(*) FROM project_id_aliases
             WHERE remote_project_id = 'BBBBBBBBBBBBBBBB'
               AND local_project_id = (SELECT id FROM projects WHERE key = 'app')",
        ),
        1
    );
    server.finish();
}

#[test]
fn prefix_only_remote_project_collision_gets_unique_prefix() {
    let env = TestEnv::new();
    let db = env.db("prefix-collision.sqlite");
    ok(env.aven(&db, ["project", "create", "App"]));
    exec_sql(&db, "UPDATE changes SET server_seq = local_seq + 90");
    let server = start_fake_sync_server(sync_response(
        1,
        [wire_change(
            "create_task",
            "task",
            "CCCCCCCCCCCCCCCC",
            json!({
                "title": "remote prefix collision",
                "project_id": "BBBBBBBBBBBBBBBB",
                "project_key": "service",
                "project_name": "service",
                "project_prefix": "APP",
            }),
        )],
    ));

    ok(env.aven(&db, ["sync", "--server", server.url()]));
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT count(*) FROM projects WHERE id = 'BBBBBBBBBBBBBBBB' AND key = 'service' AND prefix != 'APP'",
        ),
        1
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT count(*) FROM project_id_aliases WHERE remote_project_id = 'BBBBBBBBBBBBBBBB'",
        ),
        0
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT count(*) FROM tasks WHERE id = 'CCCCCCCCCCCCCCCC' AND project_id = 'BBBBBBBBBBBBBBBB'",
        ),
        1
    );
    server.finish();
}

#[test]
fn stale_project_id_alias_is_ignored_for_remote_task_create() {
    let env = TestEnv::new();
    let db = env.db("stale-alias.sqlite");
    ok(env.aven(&db, ["add", "local seed", "--project", "app"]));
    exec_sql(&db, "UPDATE changes SET server_seq = local_seq + 90");
    exec_sql(
        &db,
        "INSERT INTO project_id_aliases(workspace_id, remote_project_id, local_project_id)
         VALUES ('0000000000000000', 'BBBBBBBBBBBBBBBB', 'DDDDDDDDDDDDDDDD')",
    );
    let server = start_fake_sync_server(sync_response(
        1,
        [wire_change(
            "create_task",
            "task",
            "CCCCCCCCCCCCCCCC",
            json!({
                "title": "remote with stale alias",
                "project_id": "BBBBBBBBBBBBBBBB",
                "project_key": "remote",
                "project_name": "remote",
                "project_prefix": "REM",
            }),
        )],
    ));

    ok(env.aven(&db, ["sync", "--server", server.url()]));

    assert_eq!(
        scalar_i64(
            &db,
            "SELECT count(*) FROM projects WHERE id = 'BBBBBBBBBBBBBBBB'",
        ),
        1
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT count(*) FROM tasks WHERE id = 'CCCCCCCCCCCCCCCC' AND project_id = 'BBBBBBBBBBBBBBBB'",
        ),
        1
    );
    server.finish();
}

#[test]
fn remote_task_field_workspace_mismatch_preserves_task_and_conflicts() {
    let env = TestEnv::new();
    for (op_type, db_name) in [
        ("set_field", "field-workspace-set.sqlite"),
        ("resolve_field", "field-workspace-resolve.sqlite"),
    ] {
        let db = env.db(db_name);
        ok(env.aven(&db, ["add", "workspace scoped", "--project", "app"]));
        exec_sql(
            &db,
            "INSERT INTO workspaces(id, key, name, created_at, updated_at)
             VALUES ('1111111111111111', 'other', 'other', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        );
        exec_sql(&db, "UPDATE changes SET server_seq = local_seq + 90");
        let task_id =
            query_sql_scalar(&db, "SELECT id FROM tasks WHERE title = 'workspace scoped'");
        let server = start_fake_sync_server(sync_response(
            1,
            [json!({
                "change_id": "CCCCCCCCCCCCCCCC",
                "client_id": SYNC_CLIENT_ID,
                "local_seq": 1,
                "entity_type": "task",
                "entity_id": task_id,
                "field": "title",
                "op_type": op_type,
                "payload": {
                    "workspace_id": "1111111111111111",
                    "workspace_key": "other",
                    "value": "wrong workspace"
                },
                "base_version": null,
                "created_at": "2026-01-01T00:00:01Z",
                "server_seq": 1,
            })],
        ));

        let error = fail(env.aven(&db, ["sync", "--server", server.url()]));
        contains_all(&error, &["error invalid-task-workspace"]);
        assert_eq!(
            query_sql_scalar(
                &db,
                "SELECT title FROM tasks WHERE title = 'workspace scoped'"
            ),
            "workspace scoped"
        );
        assert_eq!(scalar_i64(&db, "SELECT count(*) FROM conflicts"), 0);
        assert_eq!(meta_value(&db, "sync_cursor").as_deref(), Some("0"));
        server.finish();
    }
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

    ok(env.aven(&a, ["edit", &task_ref, "--status", "active"]));
    ok(env.aven(&b, ["edit", &task_ref, "--priority", "high"]));
    sync(&env, &a, &server);
    sync(&env, &b, &server);
    sync(&env, &a, &server);

    let merged = ok(env.aven(&a, ["show", &task_ref]));
    contains_all(&merged, &["status=active", "priority=high"]);
    contains_none(&merged, &["conflicts=yes"]);
}

#[test]
fn available_at_remote_creation_and_scalar_updates_round_trip() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("available-client-a.sqlite");
    let b = env.db("available-client-b.sqlite");

    let task_ref = extract_ref(&ok(env.aven(
        &a,
        [
            "add",
            "scheduled sync task",
            "--project",
            "app",
            "--available-at",
            "2099-01-01T00:00:00Z",
        ],
    )));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    let remote_create = ok(env.aven(&b, ["show", &task_ref]));
    contains_all(
        &remote_create,
        &["scheduled sync task", "available_at=2099-01-01T00:00:00Z"],
    );

    ok(env.aven(
        &b,
        ["edit", &task_ref, "--available-at", "2099-02-01T00:00:00Z"],
    ));
    sync(&env, &b, &server);
    sync(&env, &a, &server);
    let updated = ok(env.aven(&a, ["show", &task_ref]));
    contains_all(&updated, &["available_at=2099-02-01T00:00:00Z"]);
    contains_none(&updated, &["conflicts=yes"]);

    ok(env.aven(&a, ["edit", &task_ref, "--clear-available-at"]));
    sync(&env, &a, &server);
    sync(&env, &b, &server);
    let cleared = ok(env.aven(&b, ["show", &task_ref, "--json"]));
    let task: Value = serde_json::from_str(&cleared).unwrap();
    assert_eq!(task["available_at"], "");
}

#[test]
fn concurrent_available_at_updates_create_field_conflict() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("available-conflict-a.sqlite");
    let b = env.db("available-conflict-b.sqlite");

    let task_ref = extract_ref(&ok(env.aven(
        &a,
        [
            "add",
            "availability conflict",
            "--project",
            "app",
            "--available-at",
            "2099-01-01T00:00:00Z",
        ],
    )));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    ok(env.aven(
        &a,
        ["edit", &task_ref, "--available-at", "2099-03-01T00:00:00Z"],
    ));
    ok(env.aven(
        &b,
        ["edit", &task_ref, "--available-at", "2099-04-01T00:00:00Z"],
    ));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    let shown = ok(env.aven(&b, ["show", &task_ref]));
    contains_all(
        &shown,
        &["available_at=2099-04-01T00:00:00Z", "conflicts=yes"],
    );
    let conflict = ok(env.aven(&b, ["conflict", "show", &task_ref]));
    contains_all(
        &conflict,
        &[
            "field=available_at",
            "2099-03-01T00:00:00Z",
            "2099-04-01T00:00:00Z",
        ],
    );
}

#[test]
fn due_on_remote_creation_updates_and_clear_round_trip() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("due-client-a.sqlite");
    let b = env.db("due-client-b.sqlite");

    let task_ref = extract_ref(&ok(env.aven(
        &a,
        [
            "add",
            "deadline sync task",
            "--project",
            "app",
            "--due",
            "2099-01-01",
        ],
    )));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    let remote_create = ok(env.aven(&b, ["show", &task_ref]));
    contains_all(&remote_create, &["deadline sync task", "due_on=2099-01-01"]);

    ok(env.aven(&b, ["edit", &task_ref, "--due", "2099-02-01"]));
    sync(&env, &b, &server);
    sync(&env, &a, &server);
    let updated = ok(env.aven(&a, ["show", &task_ref]));
    contains_all(&updated, &["due_on=2099-02-01"]);
    contains_none(&updated, &["conflicts=yes"]);

    ok(env.aven(&a, ["edit", &task_ref, "--clear-due"]));
    sync(&env, &a, &server);
    sync(&env, &b, &server);
    let cleared = ok(env.aven(&b, ["show", &task_ref, "--json"]));
    let task: Value = serde_json::from_str(&cleared).unwrap();
    assert_eq!(task["due_on"], "");
}

#[test]
fn concurrent_due_on_updates_create_field_conflict() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("due-conflict-a.sqlite");
    let b = env.db("due-conflict-b.sqlite");

    let task_ref = extract_ref(&ok(env.aven(
        &a,
        [
            "add",
            "deadline conflict",
            "--project",
            "app",
            "--due",
            "2099-01-01",
        ],
    )));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    ok(env.aven(&a, ["edit", &task_ref, "--due", "2099-03-01"]));
    ok(env.aven(&b, ["edit", &task_ref, "--due", "2099-04-01"]));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    let shown = ok(env.aven(&b, ["show", &task_ref]));
    contains_all(&shown, &["due_on=2099-04-01", "conflicts=yes"]);
    let conflict = ok(env.aven(&b, ["conflict", "show", &task_ref]));
    contains_all(&conflict, &["field=due_on", "2099-03-01", "2099-04-01"]);
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
    ok(env.aven(&a, ["edit", &task_ref, "--label", "docs"]));
    ok(env.aven(&b, ["edit", &task_ref, "--label", "sync"]));
    sync(&env, &a, &server);
    sync(&env, &b, &server);
    sync(&env, &a, &server);

    let full = ok(env.aven(&a, ["show", &task_ref, "--full"]));
    contains_all(&full, &["note a", "note b", "labels=docs,sync"]);

    ok(env.aven(&a, ["edit", &task_ref, "--remove-label", "docs"]));
    ok(env.aven(&b, ["edit", &task_ref, "--label", "bug"]));
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
    let server = start_authenticated_server(&server_env);

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
    let server = start_authenticated_server(&server_env);

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
    let server = start_authenticated_server(&server_env);

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

    let mut bad_project = task_change(
        "set_field",
        json!({
            "value": "0123456789ABCDE1",
            "project_id": "FEDCBA9876543210",
            "project_key": "app",
            "project_name": "app",
            "project_prefix": "APP",
        }),
    );
    bad_project
        .as_object_mut()
        .expect("change object")
        .insert("field".to_string(), json!("project"));
    rejected_sync(&server, bad_project, "project-value-mismatch").await;

    let mut bad_status = task_change("set_field", json!({ "value": "blocked" }));
    bad_status
        .as_object_mut()
        .expect("change object")
        .insert("field".to_string(), json!("status"));
    rejected_sync(&server, bad_status, "invalid-status").await;

    let bad_priority = task_change(
        "create_task",
        create_task_payload("bad priority", "app", json!({ "priority": "soon" })),
    );
    rejected_sync(&server, bad_priority, "invalid-priority").await;

    let bad_create_status = task_change(
        "create_task",
        create_task_payload("bad status", "app", json!({ "status": "blocked" })),
    );
    rejected_sync(&server, bad_create_status, "invalid-status").await;

    let bad_source = task_change(
        "create_task",
        create_task_payload("bad source", "app", json!({ "source": "agent" })),
    );
    rejected_sync(&server, bad_source, "invalid-task-source").await;

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

#[test]
fn dependency_sync_changes_are_idempotent() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");

    let parent = extract_ref(&ok(
        env.aven(&a, ["add", "dependency parent", "--project", "app"])
    ));
    let child = extract_ref(&ok(
        env.aven(&a, ["add", "dependency child", "--project", "app"])
    ));

    sync(&env, &a, &server);
    sync(&env, &b, &server);
    sync(&env, &a, &server);

    let add_once = ok(env.aven(&a, ["dep", "add", &child, &parent]));
    contains_all(&add_once, &["dependency-added", "changed=yes"]);
    let add_twice = ok(env.aven(&a, ["dep", "add", &child, &parent]));
    contains_all(&add_twice, &["dependency-added", "changed=none"]);

    sync(&env, &a, &server);
    sync(&env, &b, &server);

    assert_eq!(
        query_sql_scalar(
            &a,
            "SELECT count(*) FROM task_dependencies
             WHERE task_id = (SELECT id FROM tasks WHERE title = 'dependency child')
               AND depends_on_task_id = (SELECT id FROM tasks WHERE title = 'dependency parent')",
        ),
        "1"
    );
    assert_eq!(
        query_sql_scalar(
            &b,
            "SELECT count(*) FROM task_dependencies
             WHERE task_id = (SELECT id FROM tasks WHERE title = 'dependency child')
               AND depends_on_task_id = (SELECT id FROM tasks WHERE title = 'dependency parent')",
        ),
        "1"
    );

    let remove_once = ok(env.aven(&a, ["dep", "remove", &child, &parent]));
    contains_all(&remove_once, &["dependency-removed", "changed=yes"]);
    let remove_twice = ok(env.aven(&a, ["dep", "remove", &child, &parent]));
    contains_all(&remove_twice, &["dependency-removed", "changed=none"]);

    sync(&env, &a, &server);
    sync(&env, &b, &server);

    assert_eq!(
        query_sql_scalar(
            &a,
            "SELECT count(*) FROM task_dependencies
             WHERE task_id = (SELECT id FROM tasks WHERE title = 'dependency child')
               AND depends_on_task_id = (SELECT id FROM tasks WHERE title = 'dependency parent')",
        ),
        "0"
    );
    assert_eq!(
        query_sql_scalar(
            &b,
            "SELECT count(*) FROM task_dependencies
             WHERE task_id = (SELECT id FROM tasks WHERE title = 'dependency child')
               AND depends_on_task_id = (SELECT id FROM tasks WHERE title = 'dependency parent')",
        ),
        "0"
    );
}

#[tokio::test]
async fn sync_server_rejects_malformed_dependency_payloads() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);

    let missing_workspace = dependency_change_with_payload(
        "dependency_add",
        "0123456789ABCDE6",
        "0123456789ABCDE2",
        json!({ "depends_on_task_id": "0123456789ABCDE3" }),
    );
    rejected_sync(&server, missing_workspace, "workspace_id").await;

    let bad_add = dependency_change(
        "dependency_add",
        "0123456789ABCDE7",
        "0123456789ABCDE2",
        "not-a-task-id",
    );
    rejected_sync(&server, bad_add, "depends_on_task_id").await;

    let bad_remove = dependency_change(
        "dependency_remove",
        "0123456789ABCDE8",
        "0123456789ABCDE3",
        "not-a-task-id",
    );
    rejected_sync(&server, bad_remove, "depends_on_task_id").await;
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

#[tokio::test]
async fn sync_server_allocates_ordered_sequences_for_large_push_batch() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let unique_count = MAX_PUSH_BATCH - 1;
    let mut changes = (0..unique_count)
        .map(|i| valid_create_project_change(&format!("{:016X}", i + 1), i as i64 + 1))
        .collect::<Vec<_>>();
    let duplicate = changes[7].clone();
    changes.push(duplicate);
    let change_ids = changes
        .iter()
        .map(|change| change["change_id"].as_str().expect("change_id").to_string())
        .collect::<Vec<_>>();

    let response = reqwest::Client::new()
        .post(format!("{}/sync", server.url))
        .json(&json!({
            "protocol_version": SYNC_PROTOCOL_VERSION,
            "client_id": "client-a",
            "after": 0,
            "pull_limit": MAX_PULL_BATCH,
            "changes": changes,
        }))
        .send()
        .await
        .expect("post sync");

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.expect("sync response json");
    let push_acks = body["push_acks"].as_array().expect("push ack array");
    assert_eq!(push_acks.len(), change_ids.len());
    for (index, ack) in push_acks.iter().enumerate() {
        assert_eq!(ack["change_id"], json!(change_ids[index]));
    }
    for (index, ack) in push_acks.iter().take(unique_count).enumerate() {
        assert_eq!(ack["server_seq"], json!(index as i64 + 1));
    }
    assert_eq!(
        push_acks.last().expect("duplicate ack")["server_seq"],
        json!(8)
    );
    assert_eq!(
        query_sql_scalar(&env.path("server.sqlite"), "SELECT count(*) FROM changes"),
        unique_count.to_string()
    );
    assert_eq!(
        query_sql_scalar(
            &env.path("server.sqlite"),
            "SELECT max(server_seq) FROM changes"
        ),
        unique_count.to_string()
    );
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
    ok(env.aven(&a, ["edit", &task_ref, "--label", "offline"]));
    ok(env.aven_stdin(&a, ["note", &task_ref, "--stdin"], "batch note\n"));

    sync(&env, &a, &server);
    sync(&env, &b, &server);

    let full = ok(env.aven(&b, ["show", &task_ref, "--full"]));
    contains_all(&full, &["offline batch", "labels=offline", "batch note"]);
}

#[test]
fn out_of_order_dependency_sync_does_not_advance_cursor() {
    let env = TestEnv::new();
    let db = env.db("dependency-out-of-order.sqlite");
    let server = start_fake_sync_server(json!({
        "protocol_version": SYNC_PROTOCOL_VERSION,
        "cursor": 3,
        "has_more": false,
        "push_acks": [],
        "changes": [
            remote_dependency_change(SYNC_DEP_CHANGE_ID, SYNC_TASK_B_ID, SYNC_TASK_A_ID, 1),
            remote_task_change(SYNC_TASK_A_CHANGE_ID, SYNC_TASK_A_ID, "remote parent", 2),
            remote_task_change(SYNC_TASK_B_CHANGE_ID, SYNC_TASK_B_ID, "remote child", 3),
        ]
    }));

    let error = fail(env.aven(&db, ["sync", "--server", server.url()]));
    contains_all(&error, &["error dependency-missing-task"]);
    assert_eq!(scalar_i64(&db, "SELECT count(*) FROM task_dependencies"), 0);
    assert_eq!(meta_value(&db, "sync_cursor").as_deref(), Some("0"));
    server.finish();
}

#[test]
fn sync_cycle_edges_converge_deterministically() {
    let env = TestEnv::new();
    let db = env.db("dependency-cycle-convergence.sqlite");
    let server = start_fake_sync_server(json!({
        "protocol_version": SYNC_PROTOCOL_VERSION,
        "cursor": 4,
        "has_more": false,
        "push_acks": [],
        "changes": [
            remote_task_change(SYNC_TASK_A_CHANGE_ID, SYNC_TASK_A_ID, "cycle task a", 1),
            remote_task_change(SYNC_TASK_B_CHANGE_ID, SYNC_TASK_B_ID, "cycle task b", 2),
            remote_dependency_change(SYNC_DEP_CHANGE_ID, SYNC_TASK_B_ID, SYNC_TASK_A_ID, 3),
            remote_dependency_change(SYNC_OPPOSITE_DEP_CHANGE_ID, SYNC_TASK_A_ID, SYNC_TASK_B_ID, 4),
        ]
    }));

    ok(env.aven(&db, ["sync", "--server", server.url()]));
    assert_eq!(
        query_sql_scalar(
            &db,
            "SELECT task_id || '>' || depends_on_task_id FROM task_dependencies",
        ),
        "AAAAAAAAAAAAAAAA>BBBBBBBBBBBBBBBB"
    );
    server.finish();
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

#[tokio::test]
async fn sync_server_rejects_oversized_push_batch() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let changes = (0..=MAX_PUSH_BATCH)
        .map(|i| valid_create_project_change(&format!("{:016X}", i + 1), i as i64 + 1))
        .collect::<Vec<_>>();

    let response = reqwest::Client::new()
        .post(format!("{}/sync", server.url))
        .json(&json!({
            "protocol_version": SYNC_PROTOCOL_VERSION,
            "client_id": "client-a",
            "after": 0,
            "pull_limit": MAX_PULL_BATCH,
            "changes": changes,
        }))
        .send()
        .await
        .expect("post oversized sync");

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body = response.text().await.expect("error body");
    contains_all(
        &body,
        &["error sync-push-too-large", "limit=256", "got=257"],
    );
}

#[tokio::test]
async fn sync_server_rejects_out_of_range_pull_limit() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);

    for limit in [0, MAX_PULL_BATCH + 1] {
        let response = reqwest::Client::new()
            .post(format!("{}/sync", server.url))
            .json(&json!({
                "protocol_version": SYNC_PROTOCOL_VERSION,
                "client_id": "client-a",
                "after": 0,
                "pull_limit": limit,
                "changes": [],
            }))
            .send()
            .await
            .expect("post sync with invalid pull limit");

        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        let body = response.text().await.expect("error body");
        contains_all(
            &body,
            &[
                "error sync-pull-limit-out-of-range",
                "min=1",
                "max=512",
                &format!("got={limit}"),
            ],
        );
    }
}

#[test]
fn sync_server_rejects_negative_after_before_changes_are_stored() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let (status, text) = post_sync_json(
        &server.url,
        &json!({
            "protocol_version": SYNC_PROTOCOL_VERSION,
            "client_id": "client-a",
            "after": -1,
            "pull_limit": MAX_PULL_BATCH,
            "changes": [task_change("create_task", json!({"title":"ignored"}))],
        })
        .to_string(),
    );
    assert_eq!(status, 400);
    contains_all(&text, &["error sync-after-out-of-range", "min=0", "got=-1"]);
    assert_eq!(
        scalar_i64(&env.path("server.sqlite"), "SELECT count(*) FROM changes"),
        0
    );
}

#[test]
fn sync_server_returns_bounded_pull_pages() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let server_db = env.path("server.sqlite");
    let values = (1..=(MAX_PULL_BATCH + 1))
        .map(|seq| {
            let payload = json!({
                "key": format!("project-{seq}"),
                "name": format!("Project {seq}"),
                "prefix": format!("P{seq}"),
                "created_at": "2026-01-01T00:00:00Z"
            })
            .to_string()
            .replace('\'', "''");
            format!(
                "('CHG{seq:013}', 'client-a', {seq}, 'project', 'PRJ{seq:013}', NULL, 'create_project', '{payload}', NULL, '2026-01-01T00:00:00Z', {seq})"
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    exec_sql(
        &server_db,
        &format!(
            "INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field, op_type, payload, base_version, created_at, server_seq) VALUES {values}"
        ),
    );

    let (status, body) = post_sync_json(
        &server.url,
        &sync_pull_request("audit-client", 0).to_string(),
    );
    assert_eq!(status, 200);
    let body: Value = serde_json::from_str(&body).expect("sync response json");
    assert_eq!(
        body["changes"].as_array().expect("changes array").len(),
        MAX_PULL_BATCH
    );
    assert_eq!(body["cursor"], json!(MAX_PULL_BATCH));
    assert_eq!(body["has_more"], json!(true));

    let (status, body) = post_sync_json(
        &server.url,
        &sync_pull_request("audit-client", MAX_PULL_BATCH).to_string(),
    );
    assert_eq!(status, 200);
    let body: Value = serde_json::from_str(&body).expect("sync response json");
    assert_eq!(body["changes"].as_array().expect("changes array").len(), 1);
    assert_eq!(body["cursor"], json!(MAX_PULL_BATCH + 1));
    assert_eq!(body["has_more"], json!(false));
}

#[test]
fn sync_server_pure_pull_succeeds_while_write_transaction_is_held() {
    use std::io::Write;

    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let server_db = env.path("server.sqlite");
    exec_sql(
        &server_db,
        "INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field, op_type, payload, base_version, created_at, server_seq)
         VALUES ('AAAAAAAAAAAAAAAA', 'client-a', 1, 'task', 'BBBBBBBBBBBBBBBB', NULL, 'create_task',
                 '{\"workspace_id\":\"0000000000000000\",\"workspace_key\":\"default\",\"title\":\"locked pull\",\"project_id\":\"0000000000000001\",\"project_key\":\"app\",\"project_name\":\"app\",\"project_prefix\":\"APP\"}',
                 NULL, '2026-01-01T00:00:01Z', 1)"
    );

    let mut locker = std::process::Command::new("sqlite3")
        .arg(&server_db)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("start sqlite lock process");
    locker
        .stdin
        .as_mut()
        .expect("sqlite stdin")
        .write_all(b"BEGIN IMMEDIATE;\n")
        .expect("begin immediate transaction");

    let (status, body) = post_sync_json(
        &server.url,
        &json!({
            "protocol_version": SYNC_PROTOCOL_VERSION,
            "client_id": "client-a",
            "after": 0,
            "pull_limit": MAX_PULL_BATCH,
            "changes": [],
        })
        .to_string(),
    );
    assert_eq!(status, 200);
    let body: Value = serde_json::from_str(&body).expect("sync response json");
    assert_eq!(body["changes"].as_array().expect("changes array").len(), 1);

    locker.kill().expect("stop sqlite lock process");
    locker.wait().expect("wait for lock process");
}

#[test]
fn push_acks_update_local_changes_without_pull_echo() {
    let env = TestEnv::new();
    let (db, push_acks) = seed_task_with_pending_push_acks(&env, "push-acks.sqlite", "acked local");
    let server = start_fake_sync_server(sync_response_with_push_acks(0, push_acks, []));

    ok(env.aven(&db, ["sync", "--server", server.url()]));
    assert_eq!(
        query_sql_scalar(&db, "SELECT count(*) FROM changes WHERE server_seq IS NULL"),
        "0"
    );
    server.finish();
}

#[test]
fn sync_client_skips_already_stored_pulled_change_in_applied_count() {
    let env = TestEnv::new();
    let db = env.db("already-stored-pull.sqlite");
    ok(env.aven(&db, ["add", "already stored", "--project", "app"]));
    exec_sql(&db, "UPDATE changes SET server_seq = local_seq");

    let change_json = query_sql_scalar(
        &db,
        "SELECT json_object(
            'change_id', change_id, 'client_id', client_id, 'local_seq', local_seq,
            'entity_type', entity_type, 'entity_id', entity_id, 'field', field,
            'op_type', op_type, 'payload', json(payload), 'base_version', base_version,
            'created_at', created_at, 'server_seq', server_seq
         ) FROM changes ORDER BY server_seq LIMIT 1",
    );
    let change: Value = serde_json::from_str(&change_json).expect("change json");
    let cursor = change["server_seq"].as_i64().expect("server seq");
    let server = start_fake_sync_server(json!({
        "protocol_version": SYNC_PROTOCOL_VERSION,
        "cursor": cursor,
        "has_more": false,
        "push_acks": [],
        "changes": [change],
    }));

    let output = ok(env.aven(&db, ["sync", "--server", server.url()]));
    contains_all(
        &output,
        &["pushed=0", "pulled=0", &format!("cursor={cursor}")],
    );
    let expected_cursor = cursor.to_string();
    assert_eq!(
        meta_value(&db, "sync_cursor").as_deref(),
        Some(expected_cursor.as_str())
    );
    server.finish();
}

#[test]
fn sync_client_preserves_existing_pulled_change_server_seq() {
    let env = TestEnv::new();
    let db = env.db("server-seq-preserve.sqlite");
    ok(env.aven(&db, ["add", "preserve seq", "--project", "app"]));
    exec_sql(&db, "UPDATE changes SET server_seq = local_seq");

    let change_id = query_sql_scalar(
        &db,
        "SELECT change_id FROM changes WHERE server_seq IS NOT NULL ORDER BY server_seq LIMIT 1",
    );
    let original_seq = query_sql_scalar(
        &db,
        &format!("SELECT server_seq FROM changes WHERE change_id = '{change_id}'"),
    );

    // Pull a change with the same change_id but different server_seq
    let same_id_change = json!({
        "change_id": change_id,
        "client_id": "remote-client",
        "local_seq": 1,
        "entity_type": "project",
        "entity_id": "AAAAAAAAAAAAAAAA",
        "field": null,
        "op_type": "project_delete",
        "payload": {
            "workspace_id": DEFAULT_WORKSPACE_ID,
            "workspace_key": "default",
            "deleted_at": "2026-01-01T00:00:01Z"
        },
        "base_version": null,
        "created_at": "2026-01-01T00:00:01Z",
        "server_seq": 999,
    });
    let server = start_fake_sync_server(json!({
        "protocol_version": SYNC_PROTOCOL_VERSION,
        "cursor": 999,
        "has_more": false,
        "push_acks": [],
        "changes": [same_id_change],
    }));

    ok(env.aven(&db, ["sync", "--server", server.url()]));
    assert_eq!(
        query_sql_scalar(
            &db,
            &format!("SELECT server_seq FROM changes WHERE change_id = '{change_id}'"),
        ),
        original_seq
    );
    server.finish();
}

#[test]
fn sync_client_rejects_duplicate_push_acks() {
    let env = TestEnv::new();
    let (db, mut push_acks) =
        seed_task_with_pending_push_acks(&env, "duplicate-ack.sqlite", "duplicate ack");
    let change_id = push_acks[0]["change_id"]
        .as_str()
        .expect("change id")
        .to_string();
    push_acks[0] = json!({ "change_id": change_id, "server_seq": 99 });
    push_acks[1] = json!({ "change_id": change_id, "server_seq": 100 });
    let server = start_fake_sync_server(sync_response_with_push_acks(0, push_acks, []));

    let error = fail(env.aven(&db, ["sync", "--server", server.url()]));
    contains_all(
        &error,
        &["error invalid-sync-response", "duplicate-push-ack"],
    );
    assert_eq!(meta_value(&db, "sync_cursor").as_deref(), Some("0"));
    server.finish();
}

#[test]
fn sync_client_rejects_non_increasing_server_seq() {
    let env = TestEnv::new();
    let db = env.db("bad-server-seq.sqlite");
    let change = remote_task_change(SYNC_TASK_A_CHANGE_ID, SYNC_TASK_A_ID, "bad seq", 1);
    let server = start_fake_sync_server(json!({
        "protocol_version": SYNC_PROTOCOL_VERSION,
        "cursor": 1,
        "has_more": false,
        "push_acks": [],
        "changes": [change, remote_task_change(SYNC_TASK_B_CHANGE_ID, SYNC_TASK_B_ID, "bad seq b", 1)]
    }));

    let error = fail(env.aven(&db, ["sync", "--server", server.url()]));
    contains_all(&error, &["error invalid-sync-response", "server-seq-order"]);
    assert_eq!(meta_value(&db, "sync_cursor").as_deref(), Some("0"));
    server.finish();
}

#[test]
fn sync_client_rejects_oversized_pull_response_before_state_change() {
    let env = TestEnv::new();
    let db = env.db("oversized-pull.sqlite");
    ok(env.aven(&db, ["list"]));
    let cursor_before = meta_value(&db, "sync_cursor");
    let changes_before = scalar_i64(&db, "SELECT count(*) FROM changes");
    let changes = (1..=(MAX_PULL_BATCH + 1))
        .map(|seq| {
            remote_task_change(
                &format!("CHG{seq:013}"),
                &format!("TSK{seq:013}"),
                "oversized pull",
                seq as i64,
            )
        })
        .collect::<Vec<_>>();
    let server = start_fake_sync_server(json!({
        "protocol_version": SYNC_PROTOCOL_VERSION,
        "cursor": (MAX_PULL_BATCH as i64) + 1,
        "has_more": false,
        "push_acks": [],
        "changes": changes,
    }));

    let error = fail(env.aven(&db, ["sync", "--server", server.url()]));
    contains_all(&error, &["error invalid-sync-response", "pull-too-large"]);
    assert_eq!(meta_value(&db, "sync_cursor"), cursor_before);
    assert_eq!(
        scalar_i64(&db, "SELECT count(*) FROM changes"),
        changes_before
    );
    server.finish();
}

#[test]
fn sync_client_rejects_cursor_mismatch_before_state_change() {
    let env = TestEnv::new();
    let db = env.db("cursor-mismatch.sqlite");
    ok(env.aven(&db, ["list"]));
    let cursor_before = meta_value(&db, "sync_cursor");
    let server = start_fake_sync_server(json!({
        "protocol_version": SYNC_PROTOCOL_VERSION,
        "cursor": 2,
        "has_more": false,
        "push_acks": [],
        "changes": [remote_task_change(SYNC_TASK_A_CHANGE_ID, SYNC_TASK_A_ID, "bad cursor", 1)],
    }));

    let error = fail(env.aven(&db, ["sync", "--server", server.url()]));
    contains_all(&error, &["error invalid-sync-response", "cursor-mismatch"]);
    assert_eq!(meta_value(&db, "sync_cursor"), cursor_before);
    assert_eq!(
        scalar_i64(&db, "SELECT count(*) FROM tasks WHERE title = 'bad cursor'",),
        0
    );
    server.finish();
}

#[test]
fn sync_client_rejects_short_has_more_page_before_state_change() {
    let env = TestEnv::new();
    let db = env.db("short-has-more.sqlite");
    ok(env.aven(&db, ["list"]));
    let cursor_before = meta_value(&db, "sync_cursor");
    let server = start_fake_sync_server(json!({
        "protocol_version": SYNC_PROTOCOL_VERSION,
        "cursor": 1,
        "has_more": true,
        "push_acks": [],
        "changes": [remote_task_change(SYNC_TASK_A_CHANGE_ID, SYNC_TASK_A_ID, "short page", 1)],
    }));

    let error = fail(env.aven(&db, ["sync", "--server", server.url()]));
    contains_all(
        &error,
        &["error invalid-sync-response", "has-more-short-page"],
    );
    assert_eq!(meta_value(&db, "sync_cursor"), cursor_before);
    assert_eq!(
        scalar_i64(&db, "SELECT count(*) FROM tasks WHERE title = 'short page'",),
        0
    );
    server.finish();
}

#[test]
fn sync_client_rejects_malformed_pull_payload_before_state_change() {
    let env = TestEnv::new();
    let db = env.db("malformed-pull-payload.sqlite");
    ok(env.aven(&db, ["list"]));
    let cursor_before = meta_value(&db, "sync_cursor");
    let changes_before = scalar_i64(&db, "SELECT count(*) FROM changes");
    let mut change = remote_task_change(SYNC_TASK_A_CHANGE_ID, SYNC_TASK_A_ID, "bad payload", 1);
    change["payload"]["priority"] = json!("soon");
    let server = start_fake_sync_server(sync_response(1, [change]));

    let error = fail(env.aven(&db, ["sync", "--server", server.url()]));
    contains_all(&error, &["error invalid-sync-change", "invalid-priority"]);
    assert_eq!(meta_value(&db, "sync_cursor"), cursor_before);
    assert_eq!(
        scalar_i64(&db, "SELECT count(*) FROM changes"),
        changes_before
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT count(*) FROM tasks WHERE title = 'bad payload'"
        ),
        0
    );
    server.finish();
}

#[test]
fn sync_client_preserves_valid_page_cursor_after_later_page_validation_failure() {
    let env = TestEnv::new();
    let db = env.db("paged-validation-failure.sqlite");
    let first_page = (1..=MAX_PULL_BATCH)
        .map(|seq| {
            remote_task_change(
                &format!("GOOD{seq:012}"),
                &format!("TASK{seq:012}"),
                "valid page",
                seq as i64,
            )
        })
        .collect::<Vec<_>>();
    let invalid_seq = (MAX_PULL_BATCH as i64) + 1;
    let second_a = remote_task_change(
        "BAD0000000000001",
        "BADTASK000000001",
        "bad page",
        invalid_seq,
    );
    let second_b = remote_task_change(
        "BAD0000000000002",
        "BADTASK000000002",
        "bad page",
        invalid_seq,
    );
    let server = start_fake_sync_server_sequence(vec![
        json!({
            "protocol_version": SYNC_PROTOCOL_VERSION,
            "cursor": MAX_PULL_BATCH,
            "has_more": true,
            "push_acks": [],
            "changes": first_page,
        }),
        json!({
            "protocol_version": SYNC_PROTOCOL_VERSION,
            "cursor": (MAX_PULL_BATCH as i64) + 1,
            "has_more": false,
            "push_acks": [],
            "changes": [second_a, second_b],
        }),
    ]);

    let error = fail(env.aven(&db, ["sync", "--server", server.url()]));
    contains_all(&error, &["error invalid-sync-response", "server-seq-order"]);
    assert_eq!(
        scalar_i64(&db, "SELECT count(*) FROM tasks WHERE title = 'valid page'"),
        MAX_PULL_BATCH as i64
    );
    assert_eq!(
        scalar_i64(&db, "SELECT count(*) FROM tasks WHERE title = 'bad page'"),
        0
    );
    let expected_cursor = MAX_PULL_BATCH.to_string();
    assert_eq!(
        meta_value(&db, "sync_cursor").as_deref(),
        Some(expected_cursor.as_str())
    );
    server.finish();
}

#[test]
fn sync_client_drains_paged_remote_changes() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");

    for i in 0..(MAX_PULL_BATCH + 5) {
        ok(env.aven(
            &a,
            ["add", &format!("paged remote {i}"), "--project", "app"],
        ));
    }
    let output_a = ok(env.aven(&a, ["sync", "--server", &server.url]));
    contains_all(&output_a, &["pushed=", "pulled=0", "cursor="]);
    let server_cursor_before_pull = query_sql_scalar(
        &env.path("server.sqlite"),
        "SELECT max(server_seq) FROM changes",
    );
    let output_b = ok(env.aven(&b, ["sync", "--server", &server.url]));
    contains_all(
        &output_b,
        &["pushed=0", &format!("pulled={server_cursor_before_pull}")],
    );

    assert_eq!(
        scalar_i64(
            &b,
            "SELECT count(*) FROM tasks WHERE title LIKE 'paged remote %'"
        ),
        (MAX_PULL_BATCH + 5) as i64
    );
    let server_cursor = query_sql_scalar(
        &env.path("server.sqlite"),
        "SELECT max(server_seq) FROM changes",
    );
    assert_eq!(
        meta_value(&b, "sync_cursor").as_deref(),
        Some(server_cursor.as_str())
    );
}

#[test]
fn sync_client_drains_paged_local_changes() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("client-a.sqlite");

    for i in 0..(MAX_PUSH_BATCH + 5) {
        ok(env.aven(&a, ["add", &format!("paged local {i}"), "--project", "app"]));
    }
    let expected_push = scalar_i64(&a, "SELECT count(*) FROM changes WHERE server_seq IS NULL");
    let output = ok(env.aven(&a, ["sync", "--server", &server.url]));
    contains_all(
        &output,
        &[&format!("pushed={expected_push}"), "pulled=0", "cursor="],
    );

    assert_eq!(
        scalar_i64(&a, "SELECT count(*) FROM changes WHERE server_seq IS NULL"),
        0
    );
    assert_eq!(
        scalar_i64(
            &env.path("server.sqlite"),
            "SELECT count(*) FROM changes WHERE server_seq IS NOT NULL",
        ),
        scalar_i64(&a, "SELECT count(*) FROM changes")
    );
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
        "error sync-protocol-unsupported client=0 server=9",
    );
}

#[test]
fn old_request_protocol_version_is_rejected_before_changes_are_stored() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let body = serde_json::json!({
        "protocol_version": 8,
        "client_id": "old-client",
        "after": 0,
        "changes": [project_change_json("old-version-change", "old-version")]
    })
    .to_string();

    assert_sync_protocol_rejected(
        &env,
        &server,
        &body,
        "error sync-protocol-unsupported client=8 server=9",
    );
}

#[test]
fn newer_request_protocol_version_is_rejected_before_changes_are_stored() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let body = serde_json::json!({
        "protocol_version": 10,
        "client_id": "new-client",
        "after": 0,
        "changes": [project_change_json("new-version-change", "new-version")]
    })
    .to_string();

    assert_sync_protocol_rejected(
        &env,
        &server,
        &body,
        "error sync-protocol-unsupported client=10 server=9",
    );
}

#[test]
fn wrong_response_protocol_version_is_rejected() {
    let env = TestEnv::new();
    let db = env.db("client.sqlite");
    ok(env.aven(&db, ["add", "seed", "--project", "app"]));
    let changes_before = scalar_i64(&db, "SELECT count(*) FROM changes");
    let sync_cursor_before = meta_value(&db, "sync_cursor");
    let server = start_fake_sync_server(json!({
        "protocol_version": 0,
        "cursor": 7,
        "has_more": false,
        "push_acks": [],
        "changes": [project_change_json("remote-change", "rogue")]
    }));

    let error = fail(env.aven(&db, ["sync", "--server", server.url()]));
    contains_all(
        &error,
        &["error sync-protocol-unsupported client=9 server=0"],
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
    server.finish();
}

fn remote_delete_project_change(change_id: &str, project_id: &str, server_seq: i64) -> Value {
    json!({
        "change_id": change_id,
        "client_id": SYNC_CLIENT_ID,
        "local_seq": server_seq,
        "entity_type": "project",
        "entity_id": project_id,
        "field": null,
        "op_type": "project_delete",
        "payload": {
            "workspace_id": DEFAULT_WORKSPACE_ID,
            "workspace_key": "default",
            "deleted_at": format!("2026-01-01T00:00:{server_seq:02}Z")
        },
        "base_version": null,
        "created_at": format!("2026-01-01T00:00:{server_seq:02}Z"),
        "server_seq": server_seq,
    })
}

fn remote_delete_label_change(change_id: &str, label: &str, server_seq: i64) -> Value {
    json!({
        "change_id": change_id,
        "client_id": SYNC_CLIENT_ID,
        "local_seq": server_seq,
        "entity_type": "label",
        "entity_id": label,
        "field": null,
        "op_type": "label_delete",
        "payload": {
            "workspace_id": DEFAULT_WORKSPACE_ID,
            "workspace_key": "default",
            "name": label,
            "deleted_at": format!("2026-01-01T00:00:{server_seq:02}Z")
        },
        "base_version": null,
        "created_at": format!("2026-01-01T00:00:{server_seq:02}Z"),
        "server_seq": server_seq,
    })
}

fn remote_delete_note_change(
    change_id: &str,
    task_id: &str,
    note_id: &str,
    server_seq: i64,
) -> Value {
    json!({
        "change_id": change_id,
        "client_id": SYNC_CLIENT_ID,
        "local_seq": server_seq,
        "entity_type": "task",
        "entity_id": task_id,
        "field": "notes",
        "op_type": "note_delete",
        "payload": {
            "workspace_id": DEFAULT_WORKSPACE_ID,
            "workspace_key": "default",
            "note_id": note_id,
            "deleted_at": format!("2026-01-01T00:00:{server_seq:02}Z")
        },
        "base_version": null,
        "created_at": format!("2026-01-01T00:00:{server_seq:02}Z"),
        "server_seq": server_seq,
    })
}

#[test]
fn sync_deletes_converge_idempotently() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");

    ok(env.aven(&a, ["project", "create", "Delete Me"]));
    ok(env.aven(&a, ["label", "create", "obsolete"]));
    let task_ref = extract_ref(&ok(
        env.aven(&a, ["add", "delete sync seed", "--project", "app"])
    ));
    ok(env.aven(&a, ["edit", &task_ref, "--label", "obsolete"]));
    ok(env.aven_stdin(&a, ["note", &task_ref, "--stdin"], "remove this note\n"));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    let project_id = query_sql_scalar(&a, "SELECT id FROM projects WHERE key = 'delete-me'");
    let _task_id = query_sql_scalar(&a, "SELECT id FROM tasks WHERE title = 'delete sync seed'");
    let note_id = query_sql_scalar(&a, "SELECT id FROM notes WHERE body = 'remove this note\n'");

    ok(env.aven(&a, ["project", "delete", "delete-me"]));
    ok(env.aven(&a, ["label", "delete", "obsolete"]));
    ok(env.aven(&a, ["note-delete", &task_ref, &note_id]));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    // Verify client b reflects deletions
    assert_eq!(
        query_sql_scalar(
            &b,
            &format!("SELECT count(*) FROM projects WHERE id = '{project_id}' AND deleted = 1"),
        ),
        "1"
    );
    assert_eq!(
        query_sql_scalar(&b, "SELECT count(*) FROM labels WHERE name = 'obsolete'"),
        "0"
    );
    assert_eq!(
        query_sql_scalar(
            &b,
            "SELECT count(*) FROM task_labels WHERE label = 'obsolete'"
        ),
        "0"
    );
    assert_eq!(
        query_sql_scalar(
            &b,
            &format!("SELECT count(*) FROM notes WHERE id = '{note_id}'"),
        ),
        "0"
    );

    // Idempotent remote application via fake server with a fresh database
    let c = env.db("client-c.sqlite");
    ok(env.aven(&c, ["add", "seed", "--project", "app"]));
    let c_task_id = query_sql_scalar(&c, "SELECT id FROM tasks WHERE title = 'seed'");
    exec_sql(&c, "UPDATE changes SET server_seq = local_seq + 90");
    exec_sql(
        &c,
        &format!(
            "INSERT OR IGNORE INTO projects(id, workspace_id, key, name, prefix, created_at, updated_at) \
             VALUES ('{project_id}', '0000000000000000', 'delete-me', 'Delete Me', 'DEL', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
        ),
    );
    exec_sql(
        &c,
        "INSERT OR IGNORE INTO labels(workspace_id, name, created_at) \
         VALUES ('0000000000000000', 'obsolete', '2026-01-01T00:00:00Z')",
    );
    exec_sql(
        &c,
        &format!(
            "INSERT OR IGNORE INTO task_labels(workspace_id, task_id, label) \
             VALUES ('0000000000000000', '{c_task_id}', 'obsolete')"
        ),
    );
    exec_sql(
        &c,
        &format!(
            "INSERT OR IGNORE INTO notes(workspace_id, id, task_id, body, created_at, change_id) \
             VALUES ('0000000000000000', '{note_id}', '{c_task_id}', 'remove this note\n', '2026-01-01T00:00:00Z', 'PLACEHOLDER_CHANGE_ID')"
        ),
    );
    exec_sql(
        &c,
        &format!(
            "INSERT OR IGNORE INTO project_paths(workspace_id, project_id, path) \
             VALUES ('0000000000000000', '{project_id}', '/tmp/local-only-path')"
        ),
    );
    let fake = start_fake_sync_server(sync_response(
        6,
        [
            remote_delete_project_change("DELPROJECT000001", &project_id, 1),
            remote_delete_project_change("DELPROJECT000002", &project_id, 2),
            remote_delete_label_change("DELLABEL0000001", "obsolete", 3),
            remote_delete_label_change("DELLABEL0000002", "obsolete", 4),
            remote_delete_note_change("DELNOTE00000001", &c_task_id, &note_id, 5),
            remote_delete_note_change("DELNOTE00000002", &c_task_id, &note_id, 6),
        ],
    ));
    ok(env.aven(&c, ["sync", "--server", fake.url()]));
    assert_eq!(
        query_sql_scalar(
            &c,
            &format!(
                "SELECT count(*) FROM projects WHERE id = '{project_id}' AND deleted = 1 AND updated_at = '2026-01-01T00:00:02Z'"
            ),
        ),
        "1"
    );
    assert_eq!(
        query_sql_scalar(&c, "SELECT count(*) FROM labels WHERE name = 'obsolete'"),
        "0"
    );
    assert_eq!(
        query_sql_scalar(
            &c,
            "SELECT count(*) FROM task_labels WHERE label = 'obsolete'"
        ),
        "0"
    );
    assert_eq!(
        query_sql_scalar(
            &c,
            &format!("SELECT count(*) FROM notes WHERE id = '{note_id}'"),
        ),
        "0"
    );
    assert_eq!(
        query_sql_scalar(
            &c,
            &format!("SELECT queue_activity_at FROM tasks WHERE id = '{c_task_id}'"),
        ),
        "2026-01-01T00:00:05Z"
    );
    assert_eq!(
        query_sql_scalar(
            &c,
            &format!("SELECT count(*) FROM project_paths WHERE project_id = '{project_id}'"),
        ),
        "1"
    );
    fake.finish();
}

#[tokio::test]
async fn sync_server_rejects_malformed_delete_project_payloads() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);

    let valid_id = "0123456789ABCDEF";
    let bad_delete = wire_change(
        "project_delete",
        "project",
        valid_id,
        json!({ "deleted_at": "2026-01-01T00:00:00Z" }),
    );
    rejected_sync(&server, bad_delete, "workspace_id").await;

    let no_deleted = wire_change(
        "project_delete",
        "project",
        valid_id,
        json!({
            "workspace_id": "0000000000000000",
            "workspace_key": "default",
        }),
    );
    rejected_sync(&server, no_deleted, "deleted_at").await;

    let bad_deleted_at = wire_change(
        "project_delete",
        "project",
        valid_id,
        json!({
            "workspace_id": "0000000000000000",
            "workspace_key": "default",
            "deleted_at": "not-a-timestamp",
        }),
    );
    rejected_sync(&server, bad_deleted_at, "deleted_at").await;
}

#[tokio::test]
async fn sync_server_rejects_malformed_delete_label_payloads() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);

    let missing_name = wire_change(
        "label_delete",
        "label",
        "obsolete",
        json!({
            "workspace_id": "0000000000000000",
            "workspace_key": "default",
            "deleted_at": "2026-01-01T00:00:00Z",
        }),
    );
    rejected_sync(&server, missing_name, "name").await;

    let name_mismatch = wire_change(
        "label_delete",
        "label",
        "obsolete",
        json!({
            "workspace_id": "0000000000000000",
            "workspace_key": "default",
            "name": "different",
            "deleted_at": "2026-01-01T00:00:00Z",
        }),
    );
    rejected_sync(&server, name_mismatch, "label-value-mismatch").await;

    let missing_workspace = wire_change(
        "label_delete",
        "label",
        "obsolete",
        json!({
            "name": "obsolete",
            "deleted_at": "2026-01-01T00:00:00Z",
        }),
    );
    rejected_sync(&server, missing_workspace, "workspace_id").await;

    let bad_deleted_at = wire_change(
        "label_delete",
        "label",
        "obsolete",
        json!({
            "workspace_id": "0000000000000000",
            "workspace_key": "default",
            "name": "obsolete",
            "deleted_at": "not-a-timestamp",
        }),
    );
    rejected_sync(&server, bad_deleted_at, "deleted_at").await;
}

#[tokio::test]
async fn sync_server_rejects_malformed_delete_note_payloads() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);

    rejected_sync(
        &server,
        note_delete_change(json!({
            "workspace_id": "0000000000000000",
            "workspace_key": "default",
            "deleted_at": "2026-01-01T00:00:00Z",
        })),
        "note_id",
    )
    .await;

    rejected_sync(
        &server,
        note_delete_change(json!({
            "workspace_id": "0000000000000000",
            "workspace_key": "default",
            "note_id": "not-a-valid-id",
            "deleted_at": "2026-01-01T00:00:00Z",
        })),
        "note_id",
    )
    .await;

    rejected_sync(
        &server,
        note_delete_change(json!({
            "note_id": "0123456789ABCDEF",
            "deleted_at": "2026-01-01T00:00:00Z",
        })),
        "workspace_id",
    )
    .await;

    let mut wrong_field = note_delete_change(json!({
        "workspace_id": "0000000000000000",
        "workspace_key": "default",
        "note_id": "0123456789ABCDEF",
        "deleted_at": "2026-01-01T00:00:00Z",
    }));
    wrong_field
        .as_object_mut()
        .expect("change object")
        .insert("field".to_string(), json!("description"));
    rejected_sync(&server, wrong_field, "field=notes").await;

    rejected_sync(
        &server,
        note_delete_change(json!({
            "workspace_id": "0000000000000000",
            "workspace_key": "default",
            "note_id": "0123456789ABCDEF",
            "deleted_at": "not-a-timestamp",
        })),
        "deleted_at",
    )
    .await;
}

#[tokio::test]
async fn sync_server_rejects_malformed_attachment_payloads() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);

    let mut bad_field = attachment_add_change(attachment_change_payload(json!({})));
    bad_field
        .as_object_mut()
        .expect("change object")
        .insert("field".to_string(), json!("description"));
    rejected_sync(&server, bad_field, "field=attachments").await;

    let missing_workspace = attachment_add_change(attachment_change_payload(json!({
        "workspace_id": null
    })));
    rejected_sync(&server, missing_workspace, "workspace_id").await;

    let bad_entity = wire_change(
        "attachment_add",
        "task",
        "not-a-task-id",
        attachment_change_payload(json!({})),
    );
    rejected_sync(&server, bad_entity, "entity_id").await;

    rejected_sync(
        &server,
        attachment_add_change(attachment_change_payload(
            json!({ "attachment_id": "short" }),
        )),
        "attachment_id",
    )
    .await;
    rejected_sync(
        &server,
        attachment_add_change(attachment_change_payload(json!({ "sha256": "short" }))),
        "invalid-sha256",
    )
    .await;
    rejected_sync(
        &server,
        attachment_add_change(attachment_change_payload(
            json!({ "sha256": "ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789" }),
        )),
        "invalid-sha256",
    )
    .await;
    rejected_sync(
        &server,
        attachment_add_change(attachment_change_payload(json!({ "byte_size": "12" }))),
        "byte_size",
    )
    .await;
    rejected_sync(
        &server,
        attachment_add_change(attachment_change_payload(json!({ "byte_size": -1 }))),
        "invalid-attachment-size",
    )
    .await;
    rejected_sync(
        &server,
        attachment_add_change(attachment_change_payload(json!({ "byte_size": 26214401 }))),
        "invalid-attachment-size",
    )
    .await;
    rejected_sync(
        &server,
        attachment_add_change(attachment_change_payload(
            json!({ "media_type": "image/svg+xml" }),
        )),
        "unsupported-attachment-media-type",
    )
    .await;
    rejected_sync(
        &server,
        attachment_add_change(attachment_change_payload(
            json!({ "filename": "bad/name.png" }),
        )),
        "invalid-attachment-filename",
    )
    .await;
    rejected_sync(
        &server,
        attachment_add_change(attachment_change_payload(json!({ "alt_text": "bad\nalt" }))),
        "invalid-attachment-alt-text",
    )
    .await;
    rejected_sync(
        &server,
        attachment_add_change(attachment_change_payload(
            json!({ "width": 320, "height": null }),
        )),
        "invalid-attachment-dimensions",
    )
    .await;
    rejected_sync(
        &server,
        attachment_add_change(attachment_change_payload(
            json!({ "width": 0, "height": 240 }),
        )),
        "invalid-attachment-dimensions",
    )
    .await;
    rejected_sync(
        &server,
        attachment_add_change(attachment_change_payload(json!({ "created_at": "today" }))),
        "invalid-timestamp",
    )
    .await;
    rejected_sync(
        &server,
        attachment_delete_change("7KQ9A1X4MV2P8D6R", "today"),
        "invalid-timestamp",
    )
    .await;

    let mut missing_delete_field =
        attachment_delete_change("7KQ9A1X4MV2P8D6R", "2026-01-01T00:00:00Z");
    missing_delete_field
        .as_object_mut()
        .expect("change object")
        .insert("field".to_string(), Value::Null);
    rejected_sync(&server, missing_delete_field, "field=attachments").await;
}

#[test]
fn attachment_metadata_and_blobs_round_trip_through_real_sync_server() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a_dir = env.path("attachment-client-a");
    let b_dir = env.path("attachment-client-b");
    std::fs::create_dir_all(&a_dir).expect("create client a dir");
    std::fs::create_dir_all(&b_dir).expect("create client b dir");
    let a = a_dir.join("db.sqlite");
    let b = b_dir.join("db.sqlite");

    let task_ref = extract_ref(&ok(
        env.aven(&a, ["add", "attachment target", "--project", "app"])
    ));
    let task_id = local_task_id(&a, "attachment target");
    seed_local_attachment_change(&a, &task_id);
    let push_output = ok(env.aven(&a, ["sync", "--server", &server.url]));
    contains_all(&push_output, &["blob_uploaded=1", "blob_downloaded=0"]);
    let pull_output = ok(env.aven(&b, ["sync", "--server", &server.url]));
    contains_all(&pull_output, &["blob_uploaded=0", "blob_downloaded=1"]);

    assert_eq!(scalar_i64(&b, "SELECT count(*) FROM task_attachments"), 1);
    assert_eq!(scalar_i64(&b, "SELECT count(*) FROM blob_inventory"), 1);
    let json_output = ok(env.aven(&b, ["attachment", "list", &task_ref, "--json"]));
    contains_all(&json_output, &["attachment_id", "has_blob"]);
    contains_none(&json_output, &["attachment-bytes"]);
}

#[test]
fn attachment_backlog_syncs_in_bounded_unique_hash_rounds() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("bounded-attachment-client-a.sqlite");
    let b = env.db("bounded-attachment-client-b.sqlite");
    let task_ref = extract_ref(&ok(
        env.aven(&a, ["add", "bounded attachments", "--project", "app"])
    ));

    let mut first_image = None;
    for index in 0..17 {
        let image = env.path(&format!("bounded-{index}.png"));
        std::fs::write(&image, png_bytes(index + 1, 1)).expect("write attachment image");
        ok(env.aven(
            &a,
            ["attachment", "add", &task_ref, image.to_str().unwrap()],
        ));
        if index == 0 {
            first_image = Some(image);
        }
    }
    ok(env.aven(
        &a,
        [
            "attachment",
            "add",
            &task_ref,
            first_image.unwrap().to_str().unwrap(),
        ],
    ));

    let push_output = ok(env.aven(&a, ["sync", "--server", &server.url]));
    let uploads = sync_round_values(&push_output, "blob_uploaded");
    assert_eq!(uploads.iter().sum::<u64>(), 17, "{push_output}");
    assert!(uploads.iter().all(|count| *count <= 16), "{push_output}");
    contains_all(
        &push_output,
        &["blob_upload_remaining=1", "complete=false", "complete=true"],
    );
    contains_none(&push_output, &["bounded attachments", "bounded-0.png"]);

    let pull_output = ok(env.aven(&b, ["sync", "--server", &server.url]));
    let downloads = sync_round_values(&pull_output, "blob_downloaded");
    assert_eq!(downloads.iter().sum::<u64>(), 17, "{pull_output}");
    assert!(downloads.iter().all(|count| *count <= 16), "{pull_output}");
    contains_all(
        &pull_output,
        &[
            "blob_download_remaining=1",
            "complete=false",
            "complete=true",
        ],
    );
    contains_none(&pull_output, &["bounded attachments", "bounded-0.png"]);
    assert_eq!(scalar_i64(&b, "SELECT count(*) FROM task_attachments"), 18);
    assert_eq!(scalar_i64(&b, "SELECT count(*) FROM blob_inventory"), 17);
}

#[test]
fn server_workspace_quota_rejects_blob_before_attachment_metadata() {
    let env = TestEnv::new();
    env.write_config("local:\n  attachment_lifecycle:\n    server_workspace_quota_bytes: 1\n");
    let server = TestServer::start_configured(&env, "quota-server.sqlite");
    let db = env.db("quota-client.sqlite");
    let task_ref = extract_ref(&ok(
        env.aven(&db, ["add", "quota attachment", "--project", "app"])
    ));
    let image = env.path("quota.png");
    std::fs::write(&image, png_bytes(2, 2)).expect("write quota image");
    ok(env.aven(
        &db,
        ["attachment", "add", &task_ref, image.to_str().unwrap()],
    ));

    let error = fail(env.aven(&db, ["sync", "--server", &server.url]));

    contains_all(&error, &["attachment-quota-exceeded"]);
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT count(*) FROM changes WHERE op_type = 'attachment_add' AND server_seq IS NULL",
        ),
        1
    );
    assert_eq!(
        scalar_i64(
            &env.path("quota-server.sqlite"),
            "SELECT count(*) FROM server_blob_references",
        ),
        0
    );
}

#[test]
fn pulled_attachment_on_deleted_task_does_not_schedule_blob_download() {
    let env = TestEnv::new();
    let db = env.db("deleted-task-attachment.sqlite");
    ok(env.aven(&db, ["add", "deleted attachment task", "--project", "app"]));
    let task_id = local_task_id(&db, "deleted attachment task");
    exec_sql(
        &db,
        "UPDATE tasks SET deleted = 1; UPDATE changes SET server_seq = local_seq",
    );
    let change = remote_attachment_add("ATTACHMENTADD001", &task_id, 1);
    let server = start_fake_sync_server(sync_response(1, [change]));

    let output = ok(env.aven(&db, ["sync", "--server", server.url()]));

    contains_all(
        &output,
        &[
            "blob_downloaded=0",
            "blob_download_remaining=0",
            "complete=true",
        ],
    );
    assert_eq!(meta_value(&db, "sync_cursor").as_deref(), Some("1"));
    assert_eq!(scalar_i64(&db, "SELECT count(*) FROM blob_inventory"), 0);
    server.finish();
}

#[test]
fn download_failure_preserves_applied_metadata_and_cursor() {
    use sha2::{Digest, Sha256};
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::net::TcpListener;

    let env = TestEnv::new();
    let db = env.db("download-failure-cursor.sqlite");
    ok(env.aven(&db, ["add", "download failure task", "--project", "app"]));
    exec_sql(&db, "UPDATE changes SET server_seq = local_seq");
    let task_id = local_task_id(&db, "download failure task");
    let bytes = png_bytes(2, 2);
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let mut change = remote_attachment_add("ATTACHMENTADD001", &task_id, 1);
    change["payload"]["sha256"] = json!(sha256);
    change["payload"]["byte_size"] = json!(bytes.len());
    change["payload"]["width"] = json!(2);
    change["payload"]["height"] = json!(2);
    let response = sync_response(1, [change]).to_string();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake blob server");
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut sync_stream, _) = listener.accept().expect("accept sync request");
        let mut reader = BufReader::new(sync_stream.try_clone().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            reader.read_line(&mut line).expect("read sync request");
            if line == "\r\n" || line.is_empty() {
                break;
            }
        }
        write!(
            sync_stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            response.len(),
            response
        )
        .expect("write sync response");

        let (mut blob_stream, _) = listener.accept().expect("accept blob request");
        let mut reader = BufReader::new(blob_stream.try_clone().unwrap());
        loop {
            line.clear();
            reader.read_line(&mut line).expect("read blob request");
            if line == "\r\n" || line.is_empty() {
                break;
            }
        }
        write!(
            blob_stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ncontent-length: 3\r\nconnection: close\r\n\r\nbad"
        )
        .expect("write malformed blob response");
    });

    let error = fail(env.aven(&db, ["sync", "--server", &url]));

    contains_all(&error, &["error attachment-blob-remote-invalid"]);
    assert_eq!(meta_value(&db, "sync_cursor").as_deref(), Some("1"));
    assert_eq!(scalar_i64(&db, "SELECT count(*) FROM task_attachments"), 1);
    assert_eq!(scalar_i64(&db, "SELECT count(*) FROM blob_inventory"), 0);
    server.join().expect("fake blob server exits");
}

#[tokio::test]
async fn sync_server_rejects_attachment_metadata_when_blob_missing() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);

    let response = post_sync(
        &server,
        attachment_add_change(attachment_change_payload(json!({}))),
    )
    .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body = response.text().await.expect("error body");
    contains_all(&body, &["error attachment-blob-missing"]);
    assert_server_log_empty(&server).await;
}

#[tokio::test]
async fn sync_server_rejects_attachment_metadata_when_blob_metadata_disagrees() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    seed_available_blob_for_hash(
        &env.path("server.sqlite"),
        "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
    );

    let response = post_sync(
        &server,
        attachment_add_change(attachment_change_payload(json!({ "byte_size": 13 }))),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body = response.text().await.expect("error body");
    contains_all(&body, &["error blob-inventory-metadata-mismatch"]);
    assert_server_log_empty(&server).await;
}

#[test]
fn attachment_metadata_duplicate_add_is_idempotent() {
    let env = TestEnv::new();
    let db = env.db("attachment-idempotent-add.sqlite");
    ok(env.aven(&db, ["add", "target", "--project", "app"]));
    exec_sql(&db, "UPDATE changes SET server_seq = local_seq");
    let task_id = local_task_id(&db, "target");
    seed_available_blob_for_hash(
        &db,
        "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
    );
    let change1 = remote_attachment_add("ATTACHMENTADD001", &task_id, 1);
    let mut change2 = change1.clone();
    change2["change_id"] = json!("ATTACHMENTADD002");
    change2["server_seq"] = json!(2);
    let server = start_fake_sync_server(sync_response(2, [change1, change2]));

    ok(env.aven(&db, ["sync", "--server", server.url()]));

    assert_eq!(scalar_i64(&db, "SELECT count(*) FROM task_attachments"), 1);
    server.finish();
}

#[test]
fn attachment_metadata_duplicate_delete_is_idempotent() {
    let env = TestEnv::new();
    let db = env.db("attachment-idempotent-delete.sqlite");
    ok(env.aven(&db, ["add", "target", "--project", "app"]));
    exec_sql(&db, "UPDATE changes SET server_seq = local_seq");
    let task_id = local_task_id(&db, "target");
    let add = remote_attachment_add("ATTACHMENTADD001", &task_id, 1);
    let del1 = remote_attachment_delete(
        "ATTACHMENTRM0001",
        &task_id,
        "7KQ9A1X4MV2P8D6R",
        "2026-01-01T00:00:01Z",
        2,
    );
    let del2 = remote_attachment_delete(
        "ATTACHMENTRM0002",
        &task_id,
        "7KQ9A1X4MV2P8D6R",
        "2026-01-01T00:00:02Z",
        3,
    );
    let server = start_fake_sync_server(sync_response(3, [add, del1, del2]));

    ok(env.aven(&db, ["sync", "--server", server.url()]));

    assert_eq!(
        query_sql_scalar(
            &db,
            "SELECT deleted || '|' || deleted_at FROM task_attachments"
        ),
        "1|2026-01-01T00:00:01Z"
    );
    server.finish();
}

#[test]
fn attachment_metadata_conflict_rolls_back_page_and_cursor() {
    let env = TestEnv::new();
    let db = env.db("attachment-conflict.sqlite");
    ok(env.aven(&db, ["add", "target", "--project", "app"]));
    exec_sql(&db, "UPDATE changes SET server_seq = local_seq");
    let task_id = local_task_id(&db, "target");
    insert_attachment_row(
        &db,
        &task_id,
        "1111111111111111111111111111111111111111111111111111111111111111",
        false,
    );
    let change = remote_attachment_add("ATTACHMENTADD001", &task_id, 1);
    let server = start_fake_sync_server(sync_response(1, [change]));

    let error = fail(env.aven(&db, ["sync", "--server", server.url()]));

    contains_all(&error, &["error attachment-identity-conflict"]);
    assert_eq!(meta_value(&db, "sync_cursor").as_deref(), Some("0"));
    assert_eq!(
        query_sql_scalar(&db, "SELECT sha256 FROM task_attachments"),
        "1111111111111111111111111111111111111111111111111111111111111111"
    );
    server.finish();
}

#[test]
fn sync_client_rejects_malformed_attachment_response_before_state_change() {
    let env = TestEnv::new();
    let db = env.db("malformed-attachment-pull.sqlite");
    ok(env.aven(&db, ["add", "target", "--project", "app"]));
    exec_sql(&db, "UPDATE changes SET server_seq = local_seq");
    let task_id = local_task_id(&db, "target");
    let mut change = remote_attachment_add("ATTACHMENTADD001", &task_id, 1);
    change["payload"]["sha256"] = json!("short");
    let server = start_fake_sync_server(sync_response(1, [change]));

    let error = fail(env.aven(&db, ["sync", "--server", server.url()]));

    contains_all(&error, &["error invalid-sync-change", "invalid-sha256"]);
    assert_eq!(meta_value(&db, "sync_cursor").as_deref(), Some("0"));
    assert_eq!(scalar_i64(&db, "SELECT count(*) FROM task_attachments"), 0);
    server.finish();
}

#[test]
fn attachment_add_for_missing_task_fails() {
    let env = TestEnv::new();
    let db = env.db("attachment-missing-task.sqlite");
    ok(env.aven(&db, ["list"]));
    let change = remote_attachment_add("ATTACHMENTADD001", "AAAAAAAAAAAAAAAA", 1);
    let server = start_fake_sync_server(sync_response(1, [change]));

    let error = fail(env.aven(&db, ["sync", "--server", server.url()]));

    contains_all(&error, &["error attachment-missing-task"]);
    assert_eq!(meta_value(&db, "sync_cursor").as_deref(), Some("0"));
    assert_eq!(scalar_i64(&db, "SELECT count(*) FROM task_attachments"), 0);
    server.finish();
}

#[test]
fn attachment_delete_for_missing_attachment_is_idempotent() {
    let env = TestEnv::new();
    let db = env.db("attachment-missing-delete.sqlite");
    ok(env.aven(&db, ["add", "target", "--project", "app"]));
    exec_sql(&db, "UPDATE changes SET server_seq = local_seq");
    let task_id = local_task_id(&db, "target");
    let change = remote_attachment_delete(
        "ATTACHMENTRM0001",
        &task_id,
        "7KQ9A1X4MV2P8D6R",
        "2026-01-01T00:00:01Z",
        1,
    );
    let server = start_fake_sync_server(sync_response(1, [change]));

    ok(env.aven(&db, ["sync", "--server", server.url()]));

    assert_eq!(meta_value(&db, "sync_cursor").as_deref(), Some("1"));
    assert_eq!(scalar_i64(&db, "SELECT count(*) FROM task_attachments"), 0);
    server.finish();
}

#[test]
fn attachment_add_workspace_mismatch_preserves_state() {
    let env = TestEnv::new();
    let db = env.db("attachment-workspace-mismatch.sqlite");
    ok(env.aven(&db, ["add", "target", "--project", "app"]));
    exec_sql(
        &db,
        "INSERT INTO workspaces(id, key, name, created_at, updated_at)
         VALUES ('1111111111111111', 'other', 'other', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
    );
    exec_sql(&db, "UPDATE changes SET server_seq = local_seq");
    let task_id = local_task_id(&db, "target");
    let mut change = remote_attachment_add("ATTACHMENTADD001", &task_id, 1);
    change["payload"]["workspace_id"] = json!("1111111111111111");
    change["payload"]["workspace_key"] = json!("other");
    let server = start_fake_sync_server(sync_response(1, [change]));

    let error = fail(env.aven(&db, ["sync", "--server", server.url()]));

    contains_all(&error, &["error invalid-task-workspace"]);
    assert_eq!(meta_value(&db, "sync_cursor").as_deref(), Some("0"));
    assert_eq!(scalar_i64(&db, "SELECT count(*) FROM task_attachments"), 0);
    server.finish();
}

#[test]
fn attachment_add_after_delete_does_not_resurrect() {
    let env = TestEnv::new();
    let db = env.db("attachment-add-after-delete.sqlite");
    ok(env.aven(&db, ["add", "target", "--project", "app"]));
    exec_sql(&db, "UPDATE changes SET server_seq = local_seq");
    let task_id = local_task_id(&db, "target");
    insert_attachment_row(
        &db,
        &task_id,
        "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        true,
    );
    let change = remote_attachment_add("ATTACHMENTADD001", &task_id, 1);
    let server = start_fake_sync_server(sync_response(1, [change]));

    ok(env.aven(&db, ["sync", "--server", server.url()]));

    assert_eq!(
        query_sql_scalar(&db, "SELECT deleted FROM task_attachments"),
        "1"
    );
    server.finish();
}

#[test]
fn attachment_delete_identity_conflict_rejects_apply() {
    let env = TestEnv::new();
    let db = env.db("attachment-delete-conflict.sqlite");
    ok(env.aven(&db, ["add", "target1", "--project", "app"]));
    ok(env.aven(&db, ["add", "target2", "--project", "app"]));
    exec_sql(&db, "UPDATE changes SET server_seq = local_seq");
    let task1 = local_task_id(&db, "target1");
    let task2 = local_task_id(&db, "target2");
    let add = remote_attachment_add("ATTACHMENTADD001", &task1, 1);
    let del = remote_attachment_delete(
        "ATTACHMENTRM0001",
        &task2,
        "7KQ9A1X4MV2P8D6R",
        "2026-01-01T00:00:01Z",
        2,
    );
    let server = start_fake_sync_server(sync_response(2, [add, del]));

    let error = fail(env.aven(&db, ["sync", "--server", server.url()]));

    contains_all(&error, &["error attachment-identity-conflict"]);
    assert_eq!(meta_value(&db, "sync_cursor").as_deref(), Some("0"));
    assert_eq!(scalar_i64(&db, "SELECT count(*) FROM task_attachments"), 0);
    server.finish();
}

#[test]
fn sync_server_accepts_compressed_requests_and_sends_compressed_responses() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let body = json!({
        "protocol_version": SYNC_PROTOCOL_VERSION,
        "client_id": "compressed-client",
        "after": 0,
        "pull_limit": MAX_PULL_BATCH,
        "changes": []
    })
    .to_string();
    let compressed_body = gzip_encode(&body);

    let host = server
        .url
        .strip_prefix("http://")
        .expect("loopback http url");
    let raw = post_sync_raw(host, &compressed_body);
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("http response split");
    let head = String::from_utf8_lossy(&raw[..split]);
    assert!(head.starts_with("HTTP/1.0 200"), "response head: {head}");
    assert!(
        head.to_ascii_lowercase().contains("content-encoding: gzip"),
        "missing content-encoding: gzip in head:\n{head}"
    );

    let response_body = gzip_decode(&raw[(split + 4)..]);
    let response: Value = serde_json::from_str(&response_body).expect("sync response json");
    assert_eq!(response["protocol_version"], json!(SYNC_PROTOCOL_VERSION));
    assert_eq!(response["cursor"], json!(0));
    assert_eq!(
        response["changes"].as_array().expect("changes array").len(),
        0
    );
}

#[test]
fn sync_server_rejects_unauthorized_compressed_request_before_body_parse() {
    let server_env = TestEnv::new();
    let server = start_authenticated_server(&server_env);

    let host = server
        .url
        .strip_prefix("http://")
        .expect("loopback http url");
    let raw = post_sync_raw(host, b"not gzip data");
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("http response split");
    let head = String::from_utf8_lossy(&raw[..split]);
    assert!(head.starts_with("HTTP/1.0 401"), "response head: {head}");
}
