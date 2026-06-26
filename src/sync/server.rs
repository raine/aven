use std::fmt;
use std::net::IpAddr;

use anyhow::{Context, Result, bail};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::Value;
use sqlx::{SqliteConnection, SqlitePool};
use tokio::net::TcpListener;
use tracing::{error, info, warn};

use super::wire::{
    ChangeRow, ChangeWire, SYNC_PROTOCOL_VERSION, SyncRequest, SyncResponse,
    validate_sync_request_protocol_version,
};
use crate::cli::ServerArgs;
use crate::config;
use crate::db::{begin_immediate, open_db};
use crate::ids::BASE32;
use crate::signals::shutdown_signal;
use crate::task_fields::TaskField;

#[derive(Clone)]
struct ServerState {
    pool: SqlitePool,
    auth_token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BindScope {
    Loopback,
    Private,
    Public,
}

impl BindScope {
    fn classify(addr: IpAddr) -> Self {
        if addr.is_loopback() {
            Self::Loopback
        } else if is_private_addr(addr) {
            Self::Private
        } else {
            Self::Public
        }
    }
}

impl fmt::Display for BindScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Loopback => f.write_str("loopback"),
            Self::Private => f.write_str("private"),
            Self::Public => f.write_str("public"),
        }
    }
}

fn is_private_addr(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(addr) => {
            let octets = addr.octets();
            addr.is_private()
                || addr.is_link_local()
                || octets[0] == 100 && (64..=127).contains(&octets[1])
        }
        IpAddr::V6(addr) => addr.is_unique_local() || addr.is_unicast_link_local(),
    }
}

fn validate_bind_policy(
    scope: BindScope,
    unsafe_public_bind: bool,
    auth_token: Option<&str>,
) -> Result<()> {
    match scope {
        BindScope::Loopback => Ok(()),
        BindScope::Private => {
            if auth_token.is_none() {
                bail!(
                    "error private-bind-requires-auth hint=\"set sync.auth_token or bind to 127.0.0.1\""
                );
            }
            Ok(())
        }
        BindScope::Public => {
            if !unsafe_public_bind {
                bail!("error public-bind-requires --unsafe-public-bind");
            }
            if auth_token.is_none() {
                bail!("error sync-auth-token-required hint=\"set sync.auth_token in config.yaml\"");
            }
            Ok(())
        }
    }
}

pub(crate) async fn run_server(args: ServerArgs, config: config::AppConfig) -> Result<()> {
    let scope = BindScope::classify(args.bind.ip());
    let auth_token = config.sync_auth_token().map(str::to_string);
    let auth_enabled = auth_token.is_some();
    validate_bind_policy(scope, args.unsafe_public_bind, auth_token.as_deref())?;
    let pool = open_db(&args.data).await?;
    let state = ServerState { pool, auth_token };
    info!(
        bind = %args.bind,
        scope = %scope,
        auth_enabled,
        "sync server starting"
    );
    let app = Router::new()
        .route("/sync", post(sync_handler))
        .with_state(state);
    let listener = TcpListener::bind(args.bind).await?;
    let addr = listener.local_addr()?;
    if scope == BindScope::Public {
        println!("warning public bind enabled; use TLS or a reverse proxy");
    }
    println!("listening url=http://{} scope={}", addr, scope);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn sync_handler(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<SyncRequest>,
) -> Response {
    if let Err(status) = validate_auth(&state, &headers) {
        if status == StatusCode::UNAUTHORIZED {
            warn!(auth_enabled = true, "sync request unauthorized");
        }
        return status.into_response();
    }
    match handle_sync(state, request).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => {
            if err.0.is_server_error() {
                error!(status = %err.0, error = %err.1, "sync request failed");
            } else {
                warn!(status = %err.0, error = %err.1, "sync request rejected");
            }
            err.into_response()
        }
    }
}

fn internal_error(err: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}

fn invalid_sync_change(err: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, err.to_string())
}

fn validate_auth(state: &ServerState, headers: &HeaderMap) -> std::result::Result<(), StatusCode> {
    let Some(expected) = state.auth_token.as_deref() else {
        return Ok(());
    };
    let authorized = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| token == expected);
    if authorized {
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

async fn handle_sync(
    state: ServerState,
    request: SyncRequest,
) -> std::result::Result<SyncResponse, (StatusCode, String)> {
    validate_sync_request_protocol_version(request.protocol_version)
        .map_err(invalid_sync_change)?;
    let client_id = request.client_id.clone();
    let after = request.after;
    let change_count = request.changes.len();
    info!(client_id = %client_id, after, change_count, "sync request received");

    let mut conn = state.pool.acquire().await.map_err(internal_error)?;
    let mut tx = begin_immediate(&mut conn).await.map_err(internal_error)?;
    let mut accepted_count = 0_i64;
    for change in request.changes {
        validate_incoming_change(&change).map_err(invalid_sync_change)?;
        let exists = sqlx::query_scalar!(
            r#"SELECT count(*) AS "count!: i64" FROM changes WHERE change_id = ?"#,
            change.change_id
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_error)?
            > 0;
        if !exists {
            let payload = change.payload.to_string();
            sqlx::query!(
                "INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field,
                 op_type, payload, base_version, created_at, server_seq)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, (SELECT COALESCE(MAX(server_seq), 0) + 1 FROM changes))",
                change.change_id,
                change.client_id,
                change.local_seq,
                change.entity_type,
                change.entity_id,
                change.field,
                change.op_type,
                payload,
                change.base_version,
                change.created_at,
            )
            .execute(&mut *tx)
            .await
            .map_err(internal_error)?;
            accepted_count += 1;
        }
    }
    tx.commit().await.map_err(internal_error)?;
    let changes = load_server_changes_after(&mut conn, after)
        .await
        .map_err(internal_error)?;
    let cursor = changes
        .iter()
        .filter_map(|change| change.server_seq)
        .max()
        .unwrap_or(after);
    info!(
        client_id = %client_id,
        after,
        incoming = change_count,
        accepted = accepted_count,
        returned = changes.len(),
        cursor,
        "sync request completed"
    );
    Ok(SyncResponse {
        protocol_version: SYNC_PROTOCOL_VERSION,
        cursor,
        changes,
    })
}

fn validate_incoming_change(change: &ChangeWire) -> Result<()> {
    ensure_non_empty("change_id", &change.change_id)?;
    ensure_non_empty("client_id", &change.client_id)?;
    ensure_non_empty("entity_id", &change.entity_id)?;
    ensure_non_empty("op_type", &change.op_type)?;
    ensure_non_empty("entity_type", &change.entity_type)?;
    ensure_sync_id("change_id", &change.change_id)?;
    if change.server_seq.is_some() {
        bail!("error invalid-sync-change server_seq client-supplied");
    }
    if !change.payload.is_object() {
        bail!("error invalid-sync-change payload expected-object");
    }

    match change.op_type.as_str() {
        "create_workspace" => {
            ensure_entity_type(change, "workspace")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            required_string_payload("key", &change.payload)?;
            required_string_payload("name", &change.payload)?;
            required_string_payload("created_at", &change.payload)?;
        }
        "set_workspace_field" => {
            ensure_entity_type(change, "workspace")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            let field = change
                .field
                .as_deref()
                .filter(|field| !field.trim().is_empty())
                .context("error invalid-sync-change field missing")?;
            if !matches!(field, "name" | "key") {
                bail!("error invalid-sync-change field={field}");
            }
            required_string_payload("value", &change.payload)?;
        }
        "create_project" => {
            ensure_entity_type(change, "project")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            optional_workspace_payload(&change.payload)?;
            required_string_payload("key", &change.payload)?;
            required_string_payload("name", &change.payload)?;
            required_string_payload("prefix", &change.payload)?;
            required_string_payload("created_at", &change.payload)?;
        }
        "create_label" => {
            ensure_entity_type(change, "label")?;
            optional_workspace_payload(&change.payload)?;
            required_string_payload("name", &change.payload)?;
            required_string_payload("created_at", &change.payload)?;
        }
        "create_task" => {
            ensure_entity_type(change, "task")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            optional_workspace_payload(&change.payload)?;
            required_string_payload("title", &change.payload)?;
            let project_id = required_string_payload("project_id", &change.payload)?;
            ensure_sync_id("project_id", &project_id)?;
            required_string_payload("project_key", &change.payload)?;
            optional_string_payload("description", &change.payload)?;
            required_string_payload("project_name", &change.payload)?;
            required_string_payload("project_prefix", &change.payload)?;
            if let Some(status) = optional_string_payload("status", &change.payload)? {
                validate_sync_task_field_value(TaskField::Status, &status)?;
            }
            if let Some(priority) = optional_string_payload("priority", &change.payload)? {
                validate_sync_task_field_value(TaskField::Priority, &priority)?;
            }
            optional_string_array_payload("labels", &change.payload)?;
            optional_string_payload("created_at", &change.payload)?;
        }
        "set_field" | "resolve_field" => {
            ensure_entity_type(change, "task")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            optional_workspace_payload(&change.payload)?;
            let field = change
                .field
                .as_deref()
                .filter(|field| !field.trim().is_empty())
                .context("error invalid-sync-change field missing")?;
            let task_field = TaskField::parse(field)
                .ok_or_else(|| anyhow::anyhow!("error invalid-sync-change field={field}"))?;
            let value = required_string_payload("value", &change.payload)?;
            validate_sync_task_field_value(task_field, &value)?;
            if task_field == TaskField::Project {
                let project_id = required_string_payload("project_id", &change.payload)?;
                ensure_sync_id("project_id", &project_id)?;
                if value != project_id {
                    bail!("error invalid-sync-change project-value-mismatch");
                }
                required_string_payload("project_key", &change.payload)?;
                required_string_payload("project_name", &change.payload)?;
                required_string_payload("project_prefix", &change.payload)?;
            }
        }
        "label_add" | "label_remove" => {
            ensure_entity_type(change, "task")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            optional_workspace_payload(&change.payload)?;
            required_string_payload("label", &change.payload)?;
        }
        "note_add" => {
            ensure_entity_type(change, "task")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            optional_workspace_payload(&change.payload)?;
            let note_id = required_string_payload("note_id", &change.payload)?;
            ensure_sync_id("note_id", &note_id)?;
            required_string_payload("body", &change.payload)?;
            required_string_payload("created_at", &change.payload)?;
        }
        "dependency_add" | "dependency_remove" => {
            ensure_entity_type(change, "task")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            optional_workspace_payload(&change.payload)?;
            let depends_on_task_id =
                required_string_payload("depends_on_task_id", &change.payload)?;
            ensure_sync_id("depends_on_task_id", &depends_on_task_id)?;
            if change.entity_id == depends_on_task_id {
                bail!("error invalid-sync-change dependency-self");
            }
        }
        _ => bail!("error invalid-sync-change op_type={}", change.op_type),
    }
    Ok(())
}

fn validate_sync_task_field_value(field: TaskField, value: &str) -> Result<()> {
    field
        .validate_value(value)
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))
}

fn ensure_entity_type(change: &ChangeWire, expected: &str) -> Result<()> {
    if change.entity_type == expected {
        Ok(())
    } else {
        bail!(
            "error invalid-sync-change op_type={} entity_type={} expected={}",
            change.op_type,
            change.entity_type,
            expected
        )
    }
}

fn ensure_non_empty(name: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("error invalid-sync-change {name} empty");
    }
    Ok(())
}

fn ensure_sync_id(name: &str, value: &str) -> Result<()> {
    if value.len() == 16 && value.bytes().all(|byte| BASE32.contains(&byte)) {
        Ok(())
    } else {
        bail!("error invalid-sync-change {name} invalid-id");
    }
}

fn required_string_payload(key: &str, payload: &Value) -> Result<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("error invalid-sync-change payload.{key} missing"))
}

fn required_workspace_payload(payload: &Value) -> Result<()> {
    required_string_payload("workspace_id", payload)
        .and_then(|id| ensure_sync_id("workspace_id", &id))?;
    required_string_payload("workspace_key", payload)?;
    Ok(())
}

fn optional_workspace_payload(payload: &Value) -> Result<()> {
    if payload.get("workspace_id").is_none() && payload.get("workspace_key").is_none() {
        return Ok(());
    }
    required_workspace_payload(payload)
}

fn optional_string_payload(key: &str, payload: &Value) -> Result<Option<String>> {
    match payload.get(key) {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(Value::Null) | None => Ok(None),
        Some(_) => bail!("error invalid-sync-change payload.{key} invalid"),
    }
}

fn optional_string_array_payload(key: &str, payload: &Value) -> Result<()> {
    match payload.get(key) {
        Some(Value::Array(values))
            if values
                .iter()
                .all(|value| value.as_str().is_some_and(|value| !value.trim().is_empty())) =>
        {
            Ok(())
        }
        Some(Value::Null) | None => Ok(()),
        Some(_) => bail!("error invalid-sync-change payload.{key} invalid"),
    }
}

async fn load_server_changes_after(
    conn: &mut SqliteConnection,
    after: i64,
) -> Result<Vec<ChangeWire>> {
    let rows = sqlx::query_as!(
        ChangeRow,
        r#"SELECT change_id AS "change_id!: String", client_id AS "client_id!: String",
         local_seq AS "local_seq!: i64", entity_type AS "entity_type!: String",
         entity_id AS "entity_id!: String", field, op_type AS "op_type!: String",
         payload AS "payload!: String", base_version, created_at AS "created_at!: String",
         server_seq
         FROM changes WHERE server_seq > ? ORDER BY COALESCE(server_seq, local_seq), created_at"#,
        after,
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows.into_iter().map(ChangeRow::into_wire).collect())
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::{BindScope, validate_bind_policy};

    #[test]
    fn classifies_bind_scope() {
        for addr in ["127.0.0.1", "::1"] {
            assert_eq!(
                BindScope::classify(addr.parse::<IpAddr>().unwrap()),
                BindScope::Loopback
            );
        }
        for addr in [
            "10.0.0.5",
            "172.16.0.5",
            "192.168.1.5",
            "100.64.0.5",
            "100.127.255.5",
            "169.254.1.5",
            "fd00::1",
            "fe80::1",
        ] {
            assert_eq!(
                BindScope::classify(addr.parse::<IpAddr>().unwrap()),
                BindScope::Private
            );
        }
        for addr in ["8.8.8.8", "100.128.0.1", "1.1.1.1", "2001:4860:4860::8888"] {
            assert_eq!(
                BindScope::classify(addr.parse::<IpAddr>().unwrap()),
                BindScope::Public
            );
        }
    }

    #[test]
    fn bind_policy_enforces_guardrails() {
        assert!(validate_bind_policy(BindScope::Loopback, false, None).is_ok());
        assert!(validate_bind_policy(BindScope::Private, false, Some("secret")).is_ok());
        assert!(validate_bind_policy(BindScope::Public, true, Some("secret")).is_ok());

        assert_eq!(
            validate_bind_policy(BindScope::Private, false, None)
                .unwrap_err()
                .to_string(),
            "error private-bind-requires-auth hint=\"set sync.auth_token or bind to 127.0.0.1\""
        );
        assert_eq!(
            validate_bind_policy(BindScope::Public, false, Some("secret"))
                .unwrap_err()
                .to_string(),
            "error public-bind-requires --unsafe-public-bind"
        );
        assert_eq!(
            validate_bind_policy(BindScope::Public, true, None)
                .unwrap_err()
                .to_string(),
            "error sync-auth-token-required hint=\"set sync.auth_token in config.yaml\""
        );
    }
}
