use std::io::Write;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use flate2::write::GzEncoder;

use super::persistence::{ApplySyncPage, ClientSyncPage};
use super::wire::{MAX_PULL_BATCH, MAX_PUSH_BATCH, SyncResponse, sync_server_url_is_valid};
use crate::db::Database;
use crate::ids::{new_id, now};

const GZIP_THRESHOLD: usize = 256;

#[derive(Clone, PartialEq, Eq)]
pub struct SyncRequestContext {
    session_id: String,
    page: usize,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SyncHttpHeader {
    pub name: String,
    pub value: String,
}

#[derive(Clone)]
pub struct PreparedSyncRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<SyncHttpHeader>,
    pub body: Vec<u8>,
    pub context: SyncRequestContext,
}

pub struct SyncHttpResponse {
    pub status: u16,
    pub headers: Vec<SyncHttpHeader>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPageOutcome {
    pub page: usize,
    pub pushed: usize,
    pub pulled: usize,
    pub cursor: i64,
    pub complete: bool,
    pub has_more: bool,
    pub local_more: bool,
    pub request_bytes: usize,
    pub request_wire_bytes: usize,
    pub response_decoded_bytes: usize,
    pub response_compression: String,
    pub apply_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncSessionSummary {
    pub pushed: i64,
    pub pulled: usize,
    pub cursor: i64,
    pub complete: bool,
    pub pages: usize,
    pub request_bytes: usize,
    pub request_wire_bytes: usize,
    pub response_decoded_bytes: usize,
    pub response_compression: String,
    pub apply_ms: u128,
}

struct OutstandingPage {
    prepared: PreparedSyncRequest,
    page: ClientSyncPage,
    request_bytes: usize,
}

pub struct SyncSession {
    database: Database,
    server: String,
    auth_token: Option<String>,
    page_budget: Option<usize>,
    attempted_at: String,
    session_id: String,
    outstanding: Option<OutstandingPage>,
    summary: SyncSessionSummary,
    stopped: bool,
}

impl SyncSession {
    pub async fn start(
        database: Database,
        server: String,
        auth_token: Option<String>,
        page_budget: Option<usize>,
    ) -> Result<Self> {
        if !sync_server_url_is_valid(&server) {
            bail!("invalid sync server URL");
        }
        let attempted_at = now();
        database.begin_sync_attempt(attempted_at.clone()).await?;
        Ok(Self {
            database,
            server: server.trim_end_matches('/').to_string(),
            auth_token,
            page_budget,
            attempted_at,
            session_id: new_id(),
            outstanding: None,
            summary: SyncSessionSummary {
                pushed: 0,
                pulled: 0,
                cursor: 0,
                complete: false,
                pages: 0,
                request_bytes: 0,
                request_wire_bytes: 0,
                response_decoded_bytes: 0,
                response_compression: "none".to_string(),
                apply_ms: 0,
            },
            stopped: false,
        })
    }

    pub async fn prepare_request(&mut self) -> Result<Option<PreparedSyncRequest>> {
        if self.stopped {
            return Ok(None);
        }
        if let Some(outstanding) = &self.outstanding {
            return Ok(Some(outstanding.prepared.clone()));
        }

        let page = self
            .database
            .prepare_client_sync_page(self.server.clone(), MAX_PUSH_BATCH, MAX_PULL_BATCH)
            .await?;
        let body = serde_json::to_vec(&page.request).context("encode sync request")?;
        let request_bytes = body.len();
        let mut headers = vec![SyncHttpHeader {
            name: "content-type".to_string(),
            value: "application/json".to_string(),
        }];
        let body = if request_bytes > GZIP_THRESHOLD {
            headers.push(SyncHttpHeader {
                name: "content-encoding".to_string(),
                value: "gzip".to_string(),
            });
            gzip_encode(&body)?
        } else {
            body
        };
        if let Some(token) = &self.auth_token {
            headers.push(SyncHttpHeader {
                name: "authorization".to_string(),
                value: format!("Bearer {token}"),
            });
        }
        let prepared = PreparedSyncRequest {
            method: "POST".to_string(),
            url: format!("{}/sync", self.server),
            headers,
            body,
            context: SyncRequestContext {
                session_id: self.session_id.clone(),
                page: self.summary.pages + 1,
            },
        };
        self.outstanding = Some(OutstandingPage {
            prepared: prepared.clone(),
            page,
            request_bytes,
        });
        Ok(Some(prepared))
    }

    pub async fn accept_response(
        &mut self,
        context: &SyncRequestContext,
        response: SyncHttpResponse,
    ) -> Result<SyncPageOutcome> {
        let outstanding = self
            .outstanding
            .as_ref()
            .context("no outstanding sync request")?;
        validate_context(&outstanding.prepared.context, context)?;
        if !(200..300).contains(&response.status) {
            bail!("sync HTTP status {}", response.status);
        }

        let decoded_bytes = response.body.len();
        let response_compression = header_value(&response.headers, "content-encoding")
            .unwrap_or("none")
            .to_string();
        let decoded: SyncResponse =
            serde_json::from_slice(&response.body).context("decode sync response")?;
        let request = outstanding.page.request.clone();
        let pending = outstanding.page.pending;
        let request_bytes = outstanding.request_bytes;
        let request_wire_bytes = outstanding.prepared.body.len();
        let has_more = decoded.has_more;
        let cursor = decoded.cursor;
        let apply_started = Instant::now();
        let pulled = self
            .database
            .apply_client_sync_page(ApplySyncPage {
                request,
                response: decoded,
                attempted_at: self.attempted_at.clone(),
                previous_pushed: self.summary.pushed,
                previous_pulled: self.summary.pulled,
            })
            .await?;
        let apply_ms = apply_started.elapsed().as_millis();

        self.summary.pushed += pending as i64;
        self.summary.pulled += pulled;
        self.summary.cursor = cursor;
        self.summary.pages += 1;
        self.summary.request_bytes += request_bytes;
        self.summary.request_wire_bytes += request_wire_bytes;
        self.summary.response_decoded_bytes += decoded_bytes;
        self.summary.response_compression = response_compression.clone();
        self.summary.apply_ms += apply_ms;
        let local_more = pending == MAX_PUSH_BATCH;
        let complete = !local_more && !has_more;
        self.summary.complete = complete;
        self.stopped = complete
            || self
                .page_budget
                .is_some_and(|budget| self.summary.pages >= budget);
        self.outstanding = None;

        Ok(SyncPageOutcome {
            page: self.summary.pages,
            pushed: pending,
            pulled,
            cursor,
            complete,
            has_more,
            local_more,
            request_bytes,
            request_wire_bytes,
            response_decoded_bytes: decoded_bytes,
            response_compression,
            apply_ms,
        })
    }

    pub async fn fail_request(
        &mut self,
        context: &SyncRequestContext,
        error: impl Into<String>,
    ) -> Result<()> {
        let outstanding = self
            .outstanding
            .as_ref()
            .context("no outstanding sync request")?;
        validate_context(&outstanding.prepared.context, context)?;
        self.database.record_sync_error(error.into()).await?;
        self.outstanding = None;
        self.stopped = true;
        Ok(())
    }

    pub fn summary(&self) -> SyncSessionSummary {
        self.summary.clone()
    }
}

fn validate_context(expected: &SyncRequestContext, actual: &SyncRequestContext) -> Result<()> {
    if expected != actual {
        bail!("sync request context does not match the outstanding page");
    }
    Ok(())
}

fn header_value<'a>(headers: &'a [SyncHttpHeader], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str())
}

fn gzip_encode(body: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(body)
        .context("gzip encode sync request body")?;
    encoder.finish().context("finish sync request compression")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::sync::wire::SYNC_PROTOCOL_VERSION;

    async fn seed_unsynced_changes(database: &Database, count: usize) -> Vec<String> {
        let mut conn = database.acquire().await.unwrap();
        let client_id: String =
            sqlx::query_scalar("SELECT value FROM meta WHERE key = 'client_id'")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        let mut ids = Vec::with_capacity(count);
        for index in 0..count {
            let change_id = format!("fixed-change-{index:04}");
            sqlx::query(
                "INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id, \
                 op_type, payload, created_at) VALUES (?, ?, ?, 'task', ?, 'task.update', '{}', \
                 '2026-07-18T00:00:00Z')",
            )
            .bind(&change_id)
            .bind(&client_id)
            .bind(index as i64 + 1)
            .bind(format!("fixed-task-{index:04}"))
            .execute(&mut *conn)
            .await
            .unwrap();
            ids.push(change_id);
        }
        ids
    }

    fn response_bytes(cursor: i64, push_acks: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "protocol_version": SYNC_PROTOCOL_VERSION,
            "cursor": cursor,
            "has_more": false,
            "push_acks": push_acks,
            "changes": []
        }))
        .unwrap()
    }

    fn response(body: Vec<u8>) -> SyncHttpResponse {
        SyncHttpResponse {
            status: 200,
            headers: Vec::new(),
            body,
        }
    }

    fn assert_same_request(first: &PreparedSyncRequest, second: &PreparedSyncRequest) {
        assert_eq!(first.method, second.method);
        assert_eq!(first.url, second.url);
        assert!(first.headers == second.headers);
        assert_eq!(first.body, second.body);
        assert!(first.context == second.context);
    }

    #[tokio::test]
    async fn drives_multiple_pages_through_fixed_response_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(&directory.path().join("client.sqlite"))
            .await
            .unwrap();
        let change_ids = seed_unsynced_changes(&database, MAX_PUSH_BATCH).await;
        let acknowledgements = change_ids
            .iter()
            .enumerate()
            .map(|(index, change_id)| {
                json!({"change_id": change_id, "server_seq": index as i64 + 1})
            })
            .collect::<Vec<_>>();
        let first_response = response_bytes(0, json!(acknowledgements));
        let second_response = response_bytes(0, json!([]));

        let mut session = SyncSession::start(
            database.clone(),
            "https://sync.test".to_string(),
            None,
            None,
        )
        .await
        .unwrap();
        let first = session.prepare_request().await.unwrap().unwrap();
        let outcome = session
            .accept_response(&first.context, response(first_response))
            .await
            .unwrap();
        assert_eq!(outcome.pushed, MAX_PUSH_BATCH);
        assert!(!outcome.complete);
        let second = session.prepare_request().await.unwrap().unwrap();
        let outcome = session
            .accept_response(&second.context, response(second_response))
            .await
            .unwrap();
        assert_eq!(outcome.pushed, 0);
        assert!(outcome.complete);
        assert!(session.prepare_request().await.unwrap().is_none());
        assert_eq!(
            session.summary(),
            SyncSessionSummary {
                pushed: MAX_PUSH_BATCH as i64,
                pulled: 0,
                cursor: 0,
                complete: true,
                pages: 2,
                request_bytes: session.summary().request_bytes,
                request_wire_bytes: session.summary().request_wire_bytes,
                response_decoded_bytes: session.summary().response_decoded_bytes,
                response_compression: "none".to_string(),
                apply_ms: session.summary().apply_ms,
            }
        );
        assert_eq!(
            database
                .sync_persistence_status()
                .await
                .unwrap()
                .pending_changes,
            0
        );
    }

    #[tokio::test]
    async fn keeps_outstanding_page_until_acceptance_or_explicit_failure() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(&directory.path().join("client.sqlite"))
            .await
            .unwrap();
        seed_unsynced_changes(&database, 1).await;
        let mut session = SyncSession::start(
            database.clone(),
            "https://sync.test".to_string(),
            Some("secret-token".to_string()),
            None,
        )
        .await
        .unwrap();
        let first = session.prepare_request().await.unwrap().unwrap();
        let retry = session.prepare_request().await.unwrap().unwrap();
        assert_same_request(&first, &retry);

        let error = session
            .accept_response(
                &first.context,
                SyncHttpResponse {
                    status: 503,
                    headers: Vec::new(),
                    body: response_bytes(0, json!([])),
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("503"));
        let retry = session.prepare_request().await.unwrap().unwrap();
        assert_same_request(&first, &retry);
        assert_eq!(
            database
                .sync_persistence_status()
                .await
                .unwrap()
                .pending_changes,
            1
        );

        session
            .fail_request(&first.context, "transport unavailable")
            .await
            .unwrap();
        assert!(session.prepare_request().await.unwrap().is_none());
        assert_eq!(
            database.sync_persistence_status().await.unwrap().last_error,
            Some("transport unavailable".to_string())
        );
    }

    #[tokio::test]
    async fn dropping_outstanding_session_preserves_recoverable_state() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(&directory.path().join("client.sqlite"))
            .await
            .unwrap();
        let change_ids = seed_unsynced_changes(&database, 1).await;
        let mut abandoned = SyncSession::start(
            database.clone(),
            "https://sync.test".to_string(),
            None,
            None,
        )
        .await
        .unwrap();
        let abandoned_request = abandoned.prepare_request().await.unwrap().unwrap();
        drop(abandoned);
        assert_eq!(
            database
                .sync_persistence_status()
                .await
                .unwrap()
                .pending_changes,
            1
        );

        let mut recovered = SyncSession::start(
            database.clone(),
            "https://sync.test".to_string(),
            None,
            None,
        )
        .await
        .unwrap();
        let recovered_request = recovered.prepare_request().await.unwrap().unwrap();
        assert_eq!(abandoned_request.method, recovered_request.method);
        assert_eq!(abandoned_request.url, recovered_request.url);
        assert!(abandoned_request.headers == recovered_request.headers);
        assert_eq!(abandoned_request.body, recovered_request.body);
        let acknowledgements = json!([{
            "change_id": change_ids[0],
            "server_seq": 1
        }]);
        recovered
            .accept_response(
                &recovered_request.context,
                response(response_bytes(0, acknowledgements)),
            )
            .await
            .unwrap();
        assert_eq!(
            database
                .sync_persistence_status()
                .await
                .unwrap()
                .pending_changes,
            0
        );
    }

    #[tokio::test]
    async fn invalid_response_bytes_do_not_apply_or_advance_cursor() {
        let cases = [
            ("malformed", b"{".to_vec()),
            (
                "protocol",
                serde_json::to_vec(&json!({
                    "protocol_version": SYNC_PROTOCOL_VERSION + 1,
                    "cursor": 0,
                    "has_more": false,
                    "push_acks": [],
                    "changes": []
                }))
                .unwrap(),
            ),
            (
                "acknowledgement",
                response_bytes(0, json!([{"change_id": "unexpected", "server_seq": 1}])),
            ),
            (
                "oversized",
                serde_json::to_vec(&json!({
                    "protocol_version": SYNC_PROTOCOL_VERSION,
                    "cursor": 0,
                    "has_more": false,
                    "push_acks": [],
                    "changes": vec![serde_json::Value::Null; MAX_PULL_BATCH as usize + 1]
                }))
                .unwrap(),
            ),
        ];

        for (name, body) in cases {
            let directory = tempfile::tempdir().unwrap();
            let database = Database::open(&directory.path().join(format!("{name}.sqlite")))
                .await
                .unwrap();
            let mut session = SyncSession::start(
                database.clone(),
                "https://sync.test".to_string(),
                None,
                None,
            )
            .await
            .unwrap();
            let prepared = session.prepare_request().await.unwrap().unwrap();
            assert!(
                session
                    .accept_response(&prepared.context, response(body))
                    .await
                    .is_err(),
                "{name} response must fail"
            );
            let retry = session.prepare_request().await.unwrap().unwrap();
            assert_same_request(&prepared, &retry);
            assert_eq!(
                database
                    .sync_persistence_status()
                    .await
                    .unwrap()
                    .sync_cursor,
                Some("0".to_string()),
                "{name} response advanced the cursor"
            );
        }
    }

    #[tokio::test]
    async fn regressed_cursor_does_not_apply() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(&directory.path().join("client.sqlite"))
            .await
            .unwrap();
        let mut conn = database.acquire().await.unwrap();
        sqlx::query("UPDATE meta SET value = '2' WHERE key = 'sync_cursor'")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        let mut session = SyncSession::start(
            database.clone(),
            "https://sync.test".to_string(),
            None,
            None,
        )
        .await
        .unwrap();
        let prepared = session.prepare_request().await.unwrap().unwrap();
        assert!(
            session
                .accept_response(&prepared.context, response(response_bytes(1, json!([]))))
                .await
                .is_err()
        );
        assert_eq!(
            database
                .sync_persistence_status()
                .await
                .unwrap()
                .sync_cursor,
            Some("2".to_string())
        );
    }
}
