//! Device listing and removal through the CLI, with a relay in front of the
//! server that can lose the reply to one management request.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::response::Response;

use super::*;

/// Forwards every request to `upstream`. While `lose` is set, the next
/// management request reaches the server but its reply is replaced by a
/// gateway failure, so the caller cannot learn the outcome.
async fn start_relay(upstream: String, lose: Arc<AtomicBool>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let http = reqwest::Client::new();
    let app = axum::Router::new().fallback(move |request: Request| {
        let (http, upstream, lose) = (http.clone(), upstream.clone(), lose.clone());
        async move {
            let (parts, body) = request.into_parts();
            let body = to_bytes(body, usize::MAX).await.unwrap();
            let mut headers = parts.headers;
            headers.remove(header::HOST);
            let reply = http
                .request(parts.method, format!("{upstream}{}", parts.uri))
                .headers(headers)
                .body(body.clone())
                .send()
                .await
                .unwrap();
            let mut response = Response::builder().status(reply.status());
            for (name, value) in reply.headers() {
                if name != header::CONTENT_LENGTH && name != header::TRANSFER_ENCODING {
                    response = response.header(name, value);
                }
            }
            let bytes = reply.bytes().await.unwrap();
            if body.starts_with(b"{\"Manage\"") && lose.swap(false, Ordering::SeqCst) {
                return Response::builder()
                    .status(StatusCode::BAD_GATEWAY)
                    .body(Body::empty())
                    .unwrap();
            }
            response.body(Body::from(bytes)).unwrap()
        }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

async fn join_from(inviter: &Installation, joiner: &Installation) {
    let (invite, invitation, _stdout) = spawn_invite(inviter, None).await;
    let mut joined = joiner
        .run_with_input(&["sync", "join", "--yes"], &invitation)
        .await;
    // The server serves one enrollment exchange at a time, so the download
    // can meet the inviter's last poll; join resumes when rerun.
    if !joined.status.success() && failure(&joined).contains("error enrollment-busy") {
        joined = joiner
            .run_with_input(&["sync", "join", "--yes"], &invitation)
            .await;
    }
    success(&joined, &["sync", "join", "--yes"]);
    assert!(invite.wait_with_output().await.unwrap().status.success());
}

async fn devices(node: &Installation) -> serde_json::Value {
    serde_json::from_str(&node.ok(&["sync", "device", "list", "--json"]).await).unwrap()
}

/// The device IDs in a listing, sorted, and this device's ID.
fn ids(listing: &serde_json::Value) -> (Vec<String>, String) {
    let entries = listing["devices"].as_array().unwrap();
    let mut all: Vec<String> = entries
        .iter()
        .map(|entry| entry["device_id"].as_str().unwrap().to_string())
        .collect();
    all.sort();
    let current: Vec<_> = entries
        .iter()
        .filter(|entry| entry["current"] == true)
        .collect();
    assert_eq!(current.len(), 1, "{listing}");
    (all, current[0]["device_id"].as_str().unwrap().to_string())
}

#[tokio::test]
async fn cli_lists_and_removes_devices_with_resumable_removal() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let operator = Installation::new(root, "operator");
    let a = Installation::new(root, "a");
    let b = Installation::new(root, "b");
    let c = Installation::new(root, "c");
    let data = root.join("server.sqlite");
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let lose = Arc::new(AtomicBool::new(false));
    let url = start_relay(format!("http://127.0.0.1:{port}"), lose.clone()).await;

    a.ok(&["add", "Seed task"]).await;
    let error = failure(&a.run(&["sync", "device", "list"]).await);
    assert!(error.contains("sync-not-set-up"), "{error}");
    let setup = line_with(
        &operator
            .ok(&[
                "server",
                "setup",
                "--data",
                &data.display().to_string(),
                "--url",
                &url,
            ])
            .await,
        "aven-setup:",
    );
    let _server = start_server(&operator, &data, &format!("127.0.0.1:{port}")).await;
    success(
        &a.run_with_input(&["sync", "setup", "--yes"], &setup).await,
        &["sync", "setup"],
    );
    join_from(&a, &b).await;
    join_from(&b, &c).await;

    // A successful authenticated sync clears an earlier local refusal marker.
    let database = aven_core::db::Database::open(&a.db()).await.unwrap();
    database.record_sync_access_refusal().await.unwrap();
    drop(database);
    assert_eq!(status(&a).await["state"], "access-refused");
    a.ok(&["sync"]).await;
    let recovered = status(&a).await;
    assert_eq!(recovered["state"], "ready");
    assert!(recovered["access_refused_at"].is_null());
    // So does any other authenticated success, such as listing devices.
    let database = aven_core::db::Database::open(&a.db()).await.unwrap();
    database.record_sync_access_refusal().await.unwrap();
    drop(database);
    devices(&a).await;
    assert!(status(&a).await["access_refused_at"].is_null());

    // Every device sees the same verified membership and only itself as current.
    let listing = devices(&a).await;
    let object = listing.as_object().unwrap();
    let mut keys: Vec<_> = object.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        ["devices", "key_rotation_pending", "server", "version"]
    );
    assert_eq!(listing["version"], 1);
    assert_eq!(listing["server"], url.as_str());
    assert_eq!(listing["key_rotation_pending"], false);
    let (all, a_id) = ids(&listing);
    assert_eq!(all.len(), 3);
    let a_entry = listing["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["current"] == true)
        .unwrap();
    assert_eq!(a_entry["admission_sequence"], 0);
    assert!(a_entry["label"].as_str().is_some());
    let (b_all, b_id) = ids(&devices(&b).await);
    let (c_all, c_id) = ids(&devices(&c).await);
    assert_eq!((&b_all, &c_all), (&all, &all));
    assert!(a_id != b_id && b_id != c_id && a_id != c_id);
    let text = b.ok(&["sync", "device", "list"]).await;
    assert!(text.starts_with("LABEL"), "{text}");
    assert!(text.contains("ID"), "{text}");
    assert!(text.contains("this device"), "{text}");
    assert!(text.contains(&format!("{}…", &b_id[..8])), "{text}");
    assert!(text.contains(&format!("{}…", &c_id[..8])), "{text}");
    assert!(!text.contains("admission_sequence"), "{text}");
    assert!(!text.contains("key_rotation"), "{text}");

    // Targets are explicit, and refusals leave membership unchanged.
    let error = failure(&a.run(&["sync", "device", "remove", "c"]).await);
    assert!(error.contains("sync-device-id-invalid"), "{error}");
    let unknown = "ab".repeat(32);
    let error = failure(&a.run(&["sync", "device", "remove", &unknown]).await);
    assert!(error.contains("sync-device-not-found"), "{error}");
    let error = failure(&a.run(&["sync", "device", "remove", &a_id]).await);
    assert!(error.contains("sync-device-current"), "{error}");
    let output = a.run(&["sync", "device", "remove"]).await;
    assert!(!output.status.success());
    assert_eq!(ids(&devices(&a).await).0, all);

    // The server accepts the removal, but A never receives the reply.
    lose.store(true, Ordering::SeqCst);
    let error = failure(&a.run(&["sync", "device", "remove", &c_id[..4]]).await);
    assert!(error.contains("outcome-unknown"), "{error}");
    assert!(error.contains("sync-device-removal-incomplete"), "{error}");
    assert!(!lose.load(Ordering::SeqCst));
    let listing = devices(&a).await;
    assert_eq!(listing["key_rotation_pending"], true);
    assert!(!ids(&listing).0.contains(&c_id));
    let text = a.ok(&["sync", "device", "list"]).await;
    assert!(text.contains("Key update still finishing."), "{text}");
    assert!(titles(&a).await.contains(&"Seed task".to_string()));

    // A survivor's ordinary sync finishes the rotation; A's retry then
    // resolves its retained removal instead of starting another.
    b.ok(&["sync"]).await;
    assert_eq!(devices(&a).await["key_rotation_pending"], false);
    let text = a.ok(&["sync", "device", "list"]).await;
    assert!(!text.contains("Key update still finishing."), "{text}");
    let removal: serde_json::Value =
        serde_json::from_str(&a.ok(&["sync", "device", "remove", &c_id, "--json"]).await).unwrap();
    assert_eq!(
        removal,
        serde_json::json!({
            "version": 1,
            "device_id": c_id,
            "state": "complete",
            "access_revoked": true,
            "key_rotation_pending": false,
        })
    );
    let stdout = a.ok(&["sync", "device", "remove", &c_id]).await;
    assert_eq!(
        stdout,
        format!("device-removed device_id={c_id} state=complete access_revoked=true\n")
    );

    // Survivors keep syncing. The removed device keeps its local data but
    // can no longer sync or manage devices.
    a.ok(&["add", "After removal"]).await;
    converge(&[&a, &b]).await;
    assert!(titles(&b).await.contains(&"After removal".to_string()));
    // A server refusal alone does not prove removal, so the hints name it
    // as one possibility.
    let error = failure(&c.run(&["sync"]).await);
    assert!(error.contains("sync-server-refused"), "{error}");
    let refused_status = status(&c).await;
    assert_eq!(refused_status["state"], "access-refused");
    assert!(refused_status["access_refused_at"].is_string());
    let status_text = c.ok(&["sync", "status"]).await;
    assert!(
        status_text.contains("State: access unconfirmed"),
        "{status_text}"
    );
    assert!(
        status_text.contains("may have been removed"),
        "{status_text}"
    );
    let error = failure(&c.run(&["sync", "device", "list"]).await);
    assert!(error.contains("sync-server-refused"), "{error}");
    let error = failure(&c.run(&["sync", "device", "remove", &a_id]).await);
    assert!(error.contains("may have removed this device"), "{error}");
    assert!(titles(&c).await.contains(&"Seed task".to_string()));
    c.ok(&["add", "Local after removal"]).await;

    // An uninterrupted removal completes in one call; the last device
    // cannot remove itself.
    let stdout = b.ok(&["sync", "device", "remove", &a_id]).await;
    assert!(
        stdout.contains("state=complete access_revoked=true"),
        "{stdout}"
    );
    let listing = devices(&b).await;
    assert_eq!(ids(&listing).0, vec![b_id.clone()]);
    let error = failure(&b.run(&["sync", "device", "remove", &b_id]).await);
    assert!(error.contains("sync-device-current"), "{error}");
    b.ok(&["add", "Only device"]).await;
    converge(&[&b]).await;
}
