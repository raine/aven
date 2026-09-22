use super::*;
use crate::db::{
    IdentifiedChange, begin_immediate, get_meta, insert_change, insert_change_with_identity,
    set_meta,
};
use crate::sync::wire::{SyncRequest, SyncResponse};

fn decode(request: &PreparedSyncRequest) -> SyncRequest {
    use std::io::Read;
    let bytes = if request.headers.iter().any(|h| h.name == "content-encoding") {
        let mut bytes = Vec::new();
        flate2::read::GzDecoder::new(request.body.as_slice())
            .read_to_end(&mut bytes)
            .unwrap();
        bytes
    } else {
        request.body.clone()
    };
    serde_json::from_slice(&bytes).unwrap()
}

async fn reply(server: &Database, request: SyncRequest, active: u32) -> SyncHttpResponse {
    let after = request.after;
    match server.persist_test_protocol_page(request, active).await {
        Ok(page) => SyncHttpResponse {
            status: 200,
            headers: Vec::new(),
            body: serde_json::to_vec(&SyncResponse {
                protocol_version: active,
                cursor: page
                    .changes
                    .last()
                    .and_then(|change| change.server_seq)
                    .unwrap_or(after),
                has_more: page.has_more,
                push_acks: page.push_acks,
                changes: page.changes,
            })
            .unwrap(),
        },
        Err(error) => SyncHttpResponse {
            status: 400,
            headers: vec![header("content-type", "text/plain")],
            body: error.to_string().into_bytes(),
        },
    }
}

async fn session(database: &Database, latest: u32) -> SyncSession {
    let mut session = SyncSession::start(
        database.clone(),
        "https://sync.test".to_string(),
        None,
        None,
    )
    .await
    .unwrap();
    session.supported_max = latest;
    session.discovery_protocol = latest;
    session
}

async fn drain(session: &mut SyncSession, server: &Database, active: u32) -> Vec<SyncRequest> {
    let mut requests = Vec::new();
    while let Some(prepared) = session.prepare_request().await.unwrap() {
        let request = decode(&prepared);
        requests.push(request.clone());
        let response = reply(server, request, active).await;
        session
            .accept_response(&prepared.context, response)
            .await
            .unwrap();
        assert!(requests.len() < 20);
    }
    requests
}

#[tokio::test]
async fn client_generations_reopen_offline_and_preserve_pending_identity_on_cutover() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("client.sqlite");
    let server = Database::open(std::path::Path::new(":memory:"))
        .await
        .unwrap();
    let client = Database::open(&path).await.unwrap();
    let workspace = client.list_workspaces().await.unwrap().remove(0);
    client
        .create_project(&workspace, "Before update")
        .await
        .unwrap();
    drain(&mut session(&client, 18).await, &server, 18).await;
    drop(client);

    for latest in [19, 20] {
        let client = Database::open(&path).await.unwrap();
        assert_eq!(
            client
                .sync_persistence_status()
                .await
                .unwrap()
                .established_protocol,
            18
        );
        client
            .create_project(&workspace, &format!("Offline generation {latest}"))
            .await
            .unwrap();
        let requests = drain(&mut session(&client, latest).await, &server, 18).await;
        assert_eq!(requests[0].protocol_version, Some(latest));
        assert!(requests[0].changes.is_empty());
        assert_eq!(requests[1].protocol_version, Some(18));
        assert_eq!(requests[1].after, i64::MAX);
        assert_eq!(requests[2].protocol_version, Some(18));
        assert!(!requests[2].changes.is_empty());
    }

    let client = Database::open(&path).await.unwrap();
    client
        .create_project(&workspace, "Pending before cutover")
        .await
        .unwrap();
    let pending = client
        .prepare_client_sync_page(
            "https://sync.test".to_string(),
            MAX_PUSH_BATCH,
            MAX_PULL_BATCH,
        )
        .await
        .unwrap()
        .request
        .changes;
    let expected = serde_json::to_value(&pending).unwrap();
    let requests = drain(&mut session(&client, 20).await, &server, 19).await;
    assert_eq!(
        serde_json::to_value(&requests[2].changes).unwrap(),
        expected
    );
    assert_eq!(
        client
            .sync_persistence_status()
            .await
            .unwrap()
            .established_protocol,
        19
    );
    assert_eq!(
        client
            .sync_persistence_status()
            .await
            .unwrap()
            .pending_changes,
        0
    );

    // Lost acknowledgments retry canonical identities through the newer session.
    let retry = SyncRequest {
        protocol_version: Some(19),
        client_id: "retry".into(),
        after: 0,
        pull_limit: None,
        changes: pending,
    };
    let result = server.persist_test_protocol_page(retry, 19).await.unwrap();
    assert_eq!(result.accepted_count, 0);
    assert!(!result.push_acks.is_empty());
}

#[tokio::test]
async fn cutover_after_discovery_rejects_without_acknowledgment() {
    let client = Database::open(std::path::Path::new(":memory:"))
        .await
        .unwrap();
    let server = Database::open(std::path::Path::new(":memory:"))
        .await
        .unwrap();
    let workspace = client.list_workspaces().await.unwrap().remove(0);
    client.create_project(&workspace, "Pending").await.unwrap();
    let before = client.sync_persistence_status().await.unwrap();
    let mut session = session(&client, 18).await;
    let probe = session.prepare_request().await.unwrap().unwrap();
    session
        .accept_response(&probe.context, reply(&server, decode(&probe), 18).await)
        .await
        .unwrap();
    assert_eq!(
        client.sync_persistence_status().await.unwrap().sync_cursor,
        before.sync_cursor
    );
    let metadata = session.prepare_request().await.unwrap().unwrap();
    let rejected = reply(&server, decode(&metadata), 19).await;
    assert_eq!(rejected.status, 400);
    let error = session
        .accept_response(&metadata.context, rejected)
        .await
        .unwrap_err();
    assert!(error.downcast_ref::<SyncCompatibilityError>().is_some());
    assert!(
        error
            .to_string()
            .contains("local tasks and edits remain saved")
    );
    session
        .fail_request(&metadata.context, "sync response rejected")
        .await
        .unwrap();
    let after = client.sync_persistence_status().await.unwrap();
    assert_eq!(after.pending_changes, before.pending_changes);
    assert_eq!(after.sync_cursor, before.sync_cursor);
    assert_eq!(after.blocked_protocol, Some(19));
    assert_eq!(after.established_protocol, 18);
    let mut audit_request = protocol::discovery_request(19, "audit".into());
    audit_request.after = 0;
    let audit = server
        .persist_test_protocol_page(audit_request, 19)
        .await
        .unwrap();
    assert!(audit.push_acks.is_empty());
    assert!(audit.changes.is_empty());
    let recovered = drain(
        &mut super::compatibility_tests::session(&client, 20).await,
        &server,
        19,
    )
    .await;
    assert!(!recovered.last().unwrap().changes.is_empty());
    assert_eq!(
        client
            .sync_persistence_status()
            .await
            .unwrap()
            .blocked_protocol,
        None
    );
}

#[tokio::test]
async fn discovery_checks_pin_and_unknown_protocol_before_any_transfer() {
    let client = Database::open(std::path::Path::new(":memory:"))
        .await
        .unwrap();
    client
        .prepare_sync_discovery("https://sync.test")
        .await
        .unwrap();
    let mut wrong = SyncSession::start(
        client.clone(),
        "https://other.test".into(),
        Some("secret".into()),
        None,
    )
    .await
    .unwrap();
    assert!(
        wrong
            .prepare_request()
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("sync-server-changed")
    );
    let mut session = session(&client, 18).await;
    let probe = session.prepare_request().await.unwrap().unwrap();
    assert!(probe.url.ends_with("/sync"));
    assert!(decode(&probe).changes.is_empty());
    let response = SyncHttpResponse {
        status: 400,
        headers: vec![header("content-type", "text/plain")],
        body: b"error sync-protocol-unsupported client=18 server=19".to_vec(),
    };
    assert!(
        session
            .accept_response(&probe.context, response)
            .await
            .is_err()
    );
    assert_eq!(
        client
            .sync_persistence_status()
            .await
            .unwrap()
            .blocked_protocol,
        Some(19)
    );
    assert!(
        client
            .meta("sync_established_protocol")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn central_gate_rejects_future_and_unknown_operations_in_both_insert_paths() {
    let client = Database::open(std::path::Path::new(":memory:"))
        .await
        .unwrap();
    let mut conn = client.acquire_writer().await.unwrap();
    for op in ["test_protocol_19", "test_protocol_20", "unregistered"] {
        for identified in [false, true] {
            let mut tx = begin_immediate(&mut conn).await.unwrap();
            set_meta(&mut tx, "mutation_probe", "must roll back")
                .await
                .unwrap();
            let result = if identified {
                insert_change_with_identity(
                    &mut tx,
                    IdentifiedChange {
                        change_id: "AAAAAAAAAAAAAAAA",
                        entity_type: "task",
                        entity_id: "BBBBBBBBBBBBBBBB",
                        field: None,
                        op_type: op,
                        payload: serde_json::json!({}),
                        base_version: None,
                        created_at: "2026-09-16T00:00:00Z",
                    },
                )
                .await
            } else {
                insert_change(
                    &mut tx,
                    "task",
                    "BBBBBBBBBBBBBBBB",
                    None,
                    op,
                    serde_json::json!({}),
                    None,
                )
                .await
                .map(|_| ())
            };
            assert!(result.is_err());
            tx.rollback().await.unwrap();
            assert!(
                get_meta(&mut conn, "mutation_probe")
                    .await
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                get_meta(&mut conn, "local_seq").await.unwrap().as_deref(),
                Some("0")
            );
        }
    }
    for source in ["future_source", "android_v2"] {
        assert!(
            protocol::validate_operation(
                18,
                "create_task",
                None,
                &serde_json::json!({"source":source})
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn stale_behavior_context_rolls_back_page_and_mode_change() {
    let client = Database::open(std::path::Path::new(":memory:"))
        .await
        .unwrap();
    let server = Database::open(std::path::Path::new(":memory:"))
        .await
        .unwrap();
    let mut session = session(&client, 20).await;
    let probe = session.prepare_request().await.unwrap().unwrap();
    session
        .accept_response(&probe.context, reply(&server, decode(&probe), 20).await)
        .await
        .unwrap();
    let page = session.prepare_request().await.unwrap().unwrap();
    {
        let mut conn = client.acquire_writer().await.unwrap();
        protocol::establish_protocol(&mut conn, 19).await.unwrap();
    }
    let response = reply(&server, decode(&page), 20).await;
    assert!(
        session
            .accept_response(&page.context, response)
            .await
            .unwrap_err()
            .to_string()
            .contains("replica-protocol-changed")
    );
    assert_eq!(
        client
            .sync_persistence_status()
            .await
            .unwrap()
            .established_protocol,
        19
    );
}

#[tokio::test]
async fn frozen_released_protocol_18_history_replays_to_identical_state() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/sync-protocol-18.json")).unwrap();
    assert_eq!(
        fixture["source_revision"],
        "7c1f7ca415fc53468f7457bbb64bc7c8c7b42cd0"
    );
    let changes: Vec<super::super::wire::ChangeWire> =
        serde_json::from_value(fixture["changes"].clone()).unwrap();
    let client = Database::open(std::path::Path::new(":memory:"))
        .await
        .unwrap();
    let page = client
        .prepare_client_sync_page("https://sync.test".into(), MAX_PUSH_BATCH, MAX_PULL_BATCH)
        .await
        .unwrap();
    assert!(page.request.changes.is_empty());
    client
        .apply_client_sync_page(ApplySyncPage {
            request: page.request,
            sync_generation: page.sync_generation,
            response: SyncResponse {
                protocol_version: 18,
                cursor: changes.last().unwrap().server_seq.unwrap(),
                has_more: false,
                push_acks: Vec::new(),
                changes,
            },
            attempted_at: fixture["attempted_at"].as_str().unwrap().to_string(),
            previous_pushed: 0,
            previous_pulled: 0,
        })
        .await
        .unwrap();
    let mut conn = client.acquire_reader().await.unwrap();
    for (table, expected) in fixture["expected"].as_object().unwrap() {
        let rows = expected.as_array().unwrap();
        if rows.is_empty() {
            let count: i64 =
                sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
                    .fetch_one(&mut *conn)
                    .await
                    .unwrap();
            assert_eq!(count, 0, "{table}");
            continue;
        }
        let columns = rows[0]
            .as_object()
            .unwrap()
            .keys()
            .filter(|key| {
                table != "tasks" || !matches!(key.as_str(), "updated_at" | "queue_activity_at")
            })
            .map(|key| format!("'{key}', \"{key}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let actual: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT json_object({columns}) FROM {table}"
        )))
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        let mut actual = actual
            .into_iter()
            .map(|row| {
                serde_json::from_str::<serde_json::Value>(&row)
                    .unwrap()
                    .to_string()
            })
            .collect::<Vec<_>>();
        let mut expected = rows
            .iter()
            .cloned()
            .map(|mut row| {
                // Protocol 18 records receiver-local activity timestamps during apply.
                if table == "tasks" {
                    row.as_object_mut().unwrap().remove("updated_at");
                    row.as_object_mut().unwrap().remove("queue_activity_at");
                }
                row.to_string()
            })
            .collect::<Vec<_>>();
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected, "released protocol-18 table {table}");
    }
}

#[tokio::test]
async fn import_resets_relationship_and_rejects_nonbaseline_history_atomically() {
    let source = Database::open(std::path::Path::new(":memory:"))
        .await
        .unwrap();
    let target = Database::open(std::path::Path::new(":memory:"))
        .await
        .unwrap();
    let workspace = source.list_workspaces().await.unwrap().remove(0);
    source
        .create_project(&workspace, "Imported baseline")
        .await
        .unwrap();
    {
        let mut conn = source.acquire_writer().await.unwrap();
        protocol::establish_protocol(&mut conn, 19).await.unwrap();
        set_meta(&mut conn, "sync_blocked_protocol", "20")
            .await
            .unwrap();
    }
    let mut export = source.export_data(now()).await.unwrap();
    target.import_data(&export).await.unwrap();
    let status = target.sync_persistence_status().await.unwrap();
    assert_eq!(status.established_protocol, 18);
    assert_eq!(status.blocked_protocol, None);
    assert!(status.pinned_server.is_none());
    let original = target.export_data(now()).await.unwrap();
    assert_eq!(
        original.tables.changes[0].change_id,
        export.tables.changes[0].change_id
    );
    export.tables.changes[0].op_type = "test_protocol_19".to_string();
    assert!(target.import_data(&export).await.is_err());
    assert_eq!(
        serde_json::to_value(target.export_data(now()).await.unwrap().tables).unwrap(),
        serde_json::to_value(original.tables).unwrap()
    );
}

#[test]
fn pairing_uses_cumulative_client_support() {
    for active in [18, 19, 20, 21] {
        let response = SyncHttpResponse {
            status: 200,
            headers: Vec::new(),
            body: serde_json::to_vec(&SyncResponse {
                protocol_version: active,
                cursor: 0,
                has_more: false,
                push_acks: Vec::new(),
                changes: Vec::new(),
            })
            .unwrap(),
        };
        assert_eq!(
            classify_pairing_at_protocol(&response, 20),
            if active <= 20 {
                PairingConnectionValidationResponse::Accepted
            } else {
                PairingConnectionValidationResponse::IncompatibleServer
            }
        );
    }
}

#[tokio::test]
async fn attachment_inventory_waits_for_confirmed_compatibility() {
    let client = Database::open(std::path::Path::new(":memory:"))
        .await
        .unwrap();
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/sync-protocol-18.json")).unwrap();
    let attachment = fixture["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|change| change["op_type"] == "attachment_add")
        .unwrap();
    {
        let mut conn = client.acquire_writer().await.unwrap();
        let mut tx = begin_immediate(&mut conn).await.unwrap();
        insert_change(
            &mut tx,
            "task",
            attachment["entity_id"].as_str().unwrap(),
            Some("attachments"),
            "attachment_add",
            attachment["payload"].clone(),
            None,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let server = Database::open(std::path::Path::new(":memory:"))
        .await
        .unwrap();
    let mut session = session(&client, 20).await;
    for expected_protocol in [20, 18] {
        let probe = session.prepare_request().await.unwrap().unwrap();
        assert_eq!(probe.url, "https://sync.test/sync");
        assert_eq!(decode(&probe).protocol_version, Some(expected_protocol));
        assert!(decode(&probe).changes.is_empty());
        session
            .accept_response(&probe.context, reply(&server, decode(&probe), 18).await)
            .await
            .unwrap();
    }
    assert!(
        client
            .meta("sync_established_protocol")
            .await
            .unwrap()
            .is_none()
    );
    let request = session.prepare_request().await.unwrap().unwrap();
    assert!(request.url.ends_with("/sync/blobs/missing"));
    assert_eq!(
        client.meta("sync_cursor").await.unwrap().as_deref(),
        Some("0")
    );
}
