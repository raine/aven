use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Connection as _, SqliteConnection, SqlitePool};
use tokio::net::TcpListener;

use crate::choices::{PRIORITIES, STATUSES, validate_choice};
use crate::cli::{ServerArgs, SyncArgs};
use crate::config;
use crate::db::{conflict_exists, field_version, get_meta, open_db, set_field_version, set_meta};
use crate::ids::{BASE32, now};
use crate::mutation::apply_field_value;
use crate::projects::{find_project, prefix_base};
use crate::refs::get_task;
use crate::signals::shutdown_signal;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChangeWire {
    pub(crate) change_id: String,
    pub(crate) client_id: String,
    pub(crate) local_seq: i64,
    pub(crate) entity_type: String,
    pub(crate) entity_id: String,
    pub(crate) field: Option<String>,
    pub(crate) op_type: String,
    pub(crate) payload: Value,
    pub(crate) base_version: Option<String>,
    pub(crate) created_at: String,
    pub(crate) server_seq: Option<i64>,
}

#[derive(Debug)]
struct ChangeRow {
    change_id: String,
    client_id: String,
    local_seq: i64,
    entity_type: String,
    entity_id: String,
    field: Option<String>,
    op_type: String,
    payload: String,
    base_version: Option<String>,
    created_at: String,
    server_seq: Option<i64>,
}

impl ChangeRow {
    fn into_wire(self) -> ChangeWire {
        ChangeWire {
            change_id: self.change_id,
            client_id: self.client_id,
            local_seq: self.local_seq,
            entity_type: self.entity_type,
            entity_id: self.entity_id,
            field: self.field,
            op_type: self.op_type,
            payload: serde_json::from_str(&self.payload).unwrap_or(Value::Null),
            base_version: self.base_version,
            created_at: self.created_at,
            server_seq: self.server_seq,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct SyncRequest {
    client_id: String,
    after: i64,
    changes: Vec<ChangeWire>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SyncResponse {
    cursor: i64,
    changes: Vec<ChangeWire>,
}

#[derive(Clone)]
struct ServerState {
    pool: SqlitePool,
    auth_token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BindScope {
    Loopback,
    Private,
    Public,
}

impl BindScope {
    pub(crate) fn classify(addr: IpAddr) -> Self {
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
                bail!("error sync-auth-token-required hint=\"set sync.auth_token in config.toml\"");
            }
            Ok(())
        }
    }
}

pub(crate) async fn run_server(args: ServerArgs, config: config::AppConfig) -> Result<()> {
    let scope = BindScope::classify(args.bind.ip());
    let auth_token = config.sync_auth_token().map(str::to_string);
    validate_bind_policy(scope, args.unsafe_public_bind, auth_token.as_deref())?;
    let pool = open_db(&args.data).await?;
    let state = ServerState { pool, auth_token };
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
        return status.into_response();
    }
    match handle_sync(state, request).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
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
    let mut conn = state.pool.acquire().await.map_err(internal_error)?;
    let mut tx = conn.begin().await.map_err(internal_error)?;
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
        }
    }
    tx.commit().await.map_err(internal_error)?;
    let changes = load_server_changes_after(&mut conn, request.after)
        .await
        .map_err(internal_error)?;
    let cursor = changes
        .iter()
        .filter_map(|change| change.server_seq)
        .max()
        .unwrap_or(request.after);
    Ok(SyncResponse { cursor, changes })
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
        "create_project" => {
            ensure_entity_type(change, "project")?;
            required_string_payload("key", &change.payload)?;
            required_string_payload("name", &change.payload)?;
            required_string_payload("prefix", &change.payload)?;
            required_string_payload("created_at", &change.payload)?;
        }
        "create_label" => {
            ensure_entity_type(change, "label")?;
            required_string_payload("name", &change.payload)?;
            required_string_payload("created_at", &change.payload)?;
        }
        "create_task" => {
            ensure_entity_type(change, "task")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            required_string_payload("title", &change.payload)?;
            required_string_payload("project_key", &change.payload)?;
            optional_string_payload("description", &change.payload)?;
            optional_string_payload("project_name", &change.payload)?;
            optional_string_payload("project_prefix", &change.payload)?;
            if let Some(status) = optional_string_payload("status", &change.payload)? {
                validate_protocol_choice("status", &status, STATUSES)?;
            }
            if let Some(priority) = optional_string_payload("priority", &change.payload)? {
                validate_protocol_choice("priority", &priority, PRIORITIES)?;
            }
            optional_string_array_payload("labels", &change.payload)?;
            optional_string_payload("created_at", &change.payload)?;
        }
        "set_field" | "resolve_field" => {
            ensure_entity_type(change, "task")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            let field = change
                .field
                .as_deref()
                .filter(|field| !field.trim().is_empty())
                .context("error invalid-sync-change field missing")?;
            ensure_scalar_field(field)?;
            let value = required_string_payload("value", &change.payload)?;
            validate_field_value(field, &value)?;
        }
        "label_add" | "label_remove" => {
            ensure_entity_type(change, "task")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            required_string_payload("label", &change.payload)?;
        }
        "note_add" => {
            ensure_entity_type(change, "task")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            let note_id = required_string_payload("note_id", &change.payload)?;
            ensure_sync_id("note_id", &note_id)?;
            required_string_payload("body", &change.payload)?;
            required_string_payload("created_at", &change.payload)?;
        }
        _ => bail!("error invalid-sync-change op_type={}", change.op_type),
    }
    Ok(())
}

fn validate_protocol_choice(name: &str, value: &str, choices: &[&str]) -> Result<()> {
    validate_choice(name, value, choices)
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

fn ensure_scalar_field(field: &str) -> Result<()> {
    if matches!(
        field,
        "title" | "description" | "project" | "status" | "priority" | "deleted"
    ) {
        Ok(())
    } else {
        bail!("error invalid-sync-change field={field}");
    }
}

fn validate_field_value(field: &str, value: &str) -> Result<()> {
    match field {
        "status" => validate_protocol_choice("status", value, STATUSES),
        "priority" => validate_protocol_choice("priority", value, PRIORITIES),
        "deleted" if matches!(value, "0" | "1") => Ok(()),
        "deleted" => bail!("error invalid-sync-change deleted value={value}"),
        _ => Ok(()),
    }
}

fn required_string_payload(key: &str, payload: &Value) -> Result<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("error invalid-sync-change payload.{key} missing"))
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

#[derive(Debug, Clone, Copy)]
pub(crate) struct SyncSummary {
    pub(crate) pushed: i64,
    pub(crate) pulled: usize,
    pub(crate) cursor: i64,
}

pub(crate) async fn sync_client(
    conn: &mut SqliteConnection,
    args: SyncArgs,
    config: &config::AppConfig,
) -> Result<()> {
    let server = config::resolve_sync_server(args.server.as_deref(), config)?;
    let summary = run_sync_once(conn, &server, config.sync_auth_token()).await?;
    println!(
        "synced pushed={} pulled={} cursor={}",
        summary.pushed, summary.pulled, summary.cursor
    );
    Ok(())
}

pub(crate) async fn run_sync_once(
    conn: &mut SqliteConnection,
    server: &str,
    auth_token: Option<&str>,
) -> Result<SyncSummary> {
    validate_sync_server(conn, server).await?;
    let client_id = get_meta(conn, "client_id")
        .await?
        .context("missing client id")?;
    let after = get_meta(conn, "sync_cursor")
        .await?
        .unwrap_or_else(|| "0".to_string())
        .parse::<i64>()?;
    let changes = load_unsynced_changes(conn).await?;
    let pushed = changes.len() as i64;
    let url = format!("{}/sync", server.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let mut request = client.post(url).json(&SyncRequest {
        client_id,
        after,
        changes,
    });
    if let Some(token) = auth_token {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .await?
        .error_for_status()?
        .json::<SyncResponse>()
        .await?;
    let mut applied = 0;
    let mut tx = conn.begin().await?;
    for change in &response.changes {
        if change_exists(&mut tx, &change.change_id).await? {
            update_change_server_seq(&mut tx, &change.change_id, change.server_seq).await?;
            continue;
        }
        apply_remote_change(&mut tx, change).await?;
        insert_wire_change(&mut tx, change).await?;
        applied += 1;
    }
    set_meta(&mut tx, "sync_cursor", &response.cursor.to_string()).await?;
    tx.commit().await?;
    Ok(SyncSummary {
        pushed,
        pulled: applied,
        cursor: response.cursor,
    })
}

async fn validate_sync_server(conn: &mut SqliteConnection, server: &str) -> Result<()> {
    let normalized = server.trim_end_matches('/');
    if let Some(existing) = get_meta(conn, "sync_server_url").await? {
        if existing != normalized {
            bail!(
                "error sync-server-changed existing={} requested={} hint=\"use a fresh database for a different sync server\"",
                existing,
                normalized
            );
        }
    } else {
        set_meta(conn, "sync_server_url", normalized).await?;
    }
    Ok(())
}

async fn load_unsynced_changes(conn: &mut SqliteConnection) -> Result<Vec<ChangeWire>> {
    let rows = sqlx::query_as!(
        ChangeRow,
        r#"SELECT change_id AS "change_id!: String", client_id AS "client_id!: String",
         local_seq AS "local_seq!: i64", entity_type AS "entity_type!: String",
         entity_id AS "entity_id!: String", field, op_type AS "op_type!: String",
         payload AS "payload!: String", base_version, created_at AS "created_at!: String",
         server_seq
         FROM changes WHERE server_seq IS NULL ORDER BY COALESCE(server_seq, local_seq), created_at"#,
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows.into_iter().map(ChangeRow::into_wire).collect())
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

async fn change_exists(conn: &mut SqliteConnection, change_id: &str) -> Result<bool> {
    Ok(sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!: i64" FROM changes WHERE change_id = ?"#,
        change_id
    )
    .fetch_one(&mut *conn)
    .await?
        > 0)
}

async fn update_change_server_seq(
    conn: &mut SqliteConnection,
    change_id: &str,
    server_seq: Option<i64>,
) -> Result<()> {
    if let Some(server_seq) = server_seq {
        sqlx::query!(
            "UPDATE changes SET server_seq = ? WHERE change_id = ?",
            server_seq,
            change_id,
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

async fn insert_wire_change(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let payload = change.payload.to_string();
    sqlx::query!(
        "INSERT OR IGNORE INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field,
         op_type, payload, base_version, created_at, server_seq)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        change.server_seq,
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn apply_remote_change(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    match change.op_type.as_str() {
        "create_project" => {
            let key = str_payload(&change.payload, "key")?;
            let name = str_payload(&change.payload, "name")?;
            let prefix = str_payload(&change.payload, "prefix")?;
            let created_at = str_payload(&change.payload, "created_at").unwrap_or_else(|_| now());
            sqlx::query!(
                "INSERT OR IGNORE INTO projects(key, name, prefix, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?)",
                key,
                name,
                prefix,
                created_at,
                created_at,
            )
            .execute(&mut *conn)
            .await?;
        }
        "create_label" => {
            let name = str_payload(&change.payload, "name")?;
            let created_at = str_payload(&change.payload, "created_at").unwrap_or_else(|_| now());
            sqlx::query!(
                "INSERT OR IGNORE INTO labels(name, created_at) VALUES (?, ?)",
                name,
                created_at,
            )
            .execute(&mut *conn)
            .await?;
        }
        "create_task" => apply_remote_create_task(conn, change).await?,
        "set_field" => apply_remote_set_field(conn, change, false).await?,
        "resolve_field" => apply_remote_set_field(conn, change, true).await?,
        "label_add" => {
            let label = str_payload(&change.payload, "label")?;
            sqlx::query!(
                "INSERT OR IGNORE INTO labels(name, created_at) VALUES (?, ?)",
                label,
                change.created_at,
            )
            .execute(&mut *conn)
            .await?;
            sqlx::query!(
                "INSERT OR IGNORE INTO task_labels(task_id, label) VALUES (?, ?)",
                change.entity_id,
                label,
            )
            .execute(&mut *conn)
            .await?;
        }
        "label_remove" => {
            let label = str_payload(&change.payload, "label")?;
            sqlx::query!(
                "DELETE FROM task_labels WHERE task_id = ? AND label = ?",
                change.entity_id,
                label,
            )
            .execute(&mut *conn)
            .await?;
        }
        "note_add" => {
            let note_id = str_payload(&change.payload, "note_id")?;
            let body = str_payload(&change.payload, "body")?;
            let created_at = str_payload(&change.payload, "created_at")
                .unwrap_or_else(|_| change.created_at.clone());
            sqlx::query!(
                "INSERT OR IGNORE INTO notes(id, task_id, body, created_at, change_id)
                 VALUES (?, ?, ?, ?, ?)",
                note_id,
                change.entity_id,
                body,
                created_at,
                change.change_id,
            )
            .execute(&mut *conn)
            .await?;
        }
        _ => {}
    }
    Ok(())
}

async fn apply_remote_create_task(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    if sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!: i64" FROM tasks WHERE id = ?"#,
        change.entity_id
    )
    .fetch_one(&mut *conn)
    .await?
        > 0
    {
        return Ok(());
    }
    let project_key = str_payload(&change.payload, "project_key")?;
    if find_project(conn, &project_key).await?.is_none() {
        let name =
            str_payload(&change.payload, "project_name").unwrap_or_else(|_| project_key.clone());
        let prefix = str_payload(&change.payload, "project_prefix")
            .unwrap_or_else(|_| prefix_base(&project_key));
        sqlx::query!(
            "INSERT OR IGNORE INTO projects(key, name, prefix, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?)",
            project_key,
            name,
            prefix,
            change.created_at,
            change.created_at,
        )
        .execute(&mut *conn)
        .await?;
    }
    let title = str_payload(&change.payload, "title")?;
    let description = str_payload(&change.payload, "description").unwrap_or_default();
    let status = str_payload(&change.payload, "status").unwrap_or_else(|_| "inbox".to_string());
    let priority = str_payload(&change.payload, "priority").unwrap_or_else(|_| "none".to_string());
    let created_at =
        str_payload(&change.payload, "created_at").unwrap_or_else(|_| change.created_at.clone());
    sqlx::query!(
        "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        change.entity_id,
        title,
        description,
        project_key,
        status,
        priority,
        created_at,
        change.created_at,
    )
    .execute(&mut *conn)
    .await?;
    if let Some(labels) = change.payload.get("labels").and_then(Value::as_array) {
        for label in labels.iter().filter_map(Value::as_str) {
            sqlx::query!(
                "INSERT OR IGNORE INTO labels(name, created_at) VALUES (?, ?)",
                label,
                change.created_at,
            )
            .execute(&mut *conn)
            .await?;
            sqlx::query!(
                "INSERT OR IGNORE INTO task_labels(task_id, label) VALUES (?, ?)",
                change.entity_id,
                label,
            )
            .execute(&mut *conn)
            .await?;
        }
    }
    for field in [
        "title",
        "description",
        "project",
        "status",
        "priority",
        "deleted",
    ] {
        set_field_version(conn, &change.entity_id, field, &change.change_id).await?;
    }
    Ok(())
}

pub(crate) async fn apply_remote_set_field(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
    force: bool,
) -> Result<()> {
    let field = change
        .field
        .as_deref()
        .context("field change missing field")?;
    let value = str_payload(&change.payload, "value")?;
    if !force {
        let current = field_version(conn, &change.entity_id, field).await?;
        if current != change.base_version {
            create_conflict(conn, change, field, &value, current.as_deref()).await?;
            return Ok(());
        }
    }
    apply_field_value(conn, &change.entity_id, field, &value).await?;
    set_field_version(conn, &change.entity_id, field, &change.change_id).await?;
    if force {
        sqlx::query!(
            "UPDATE conflicts SET resolved = 1 WHERE task_id = ? AND field = ? AND resolved = 0",
            change.entity_id,
            field,
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

async fn create_conflict(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
    field: &str,
    remote_value: &str,
    local_change_id: Option<&str>,
) -> Result<()> {
    if conflict_exists(conn, &change.entity_id, field).await? {
        return Ok(());
    }
    let local_value = current_field_value(conn, &change.entity_id, field).await?;
    let variant_a = format!(
        "v{}",
        local_change_id
            .unwrap_or("local")
            .chars()
            .take(6)
            .collect::<String>()
    );
    let variant_b = format!("v{}", change.change_id.chars().take(6).collect::<String>());
    sqlx::query!(
        "INSERT OR IGNORE INTO conflicts(task_id, field, base_version, local_value, remote_value,
         local_change_id, remote_change_id, variant_a, variant_b, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        change.entity_id,
        field,
        change.base_version,
        local_value,
        remote_value,
        local_change_id,
        change.change_id,
        variant_a,
        variant_b,
        change.created_at,
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn current_field_value(
    conn: &mut SqliteConnection,
    task_id: &str,
    field: &str,
) -> Result<String> {
    let task = get_task(conn, task_id).await?;
    match field {
        "title" => Ok(task.title),
        "description" => Ok(task.description),
        "project" => Ok(task.project_key),
        "status" => Ok(task.status),
        "priority" => Ok(task.priority),
        "deleted" => Ok(if task.deleted { "1" } else { "0" }.to_string()),
        _ => bail!("error unknown-field field={field}"),
    }
}

fn str_payload(payload: &Value, key: &str) -> Result<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("payload missing {key}"))
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
            "error sync-auth-token-required hint=\"set sync.auth_token in config.toml\""
        );
    }
}
