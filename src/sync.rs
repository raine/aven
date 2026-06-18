use std::time::Duration;

use anyhow::{Context, Result, bail};
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::routing::post;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Connection as _, SqliteConnection, SqlitePool};
use tokio::net::TcpListener;

use crate::cli::{ServerArgs, SyncArgs};
use crate::config;
use crate::db::{conflict_exists, field_version, get_meta, open_db, set_field_version, set_meta};
use crate::ids::now;
use crate::signals::shutdown_signal;
use crate::{apply_field_value, find_project, get_task, prefix_base};

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
}

pub(crate) async fn run_server(args: ServerArgs) -> Result<()> {
    if !args.unsafe_public_bind && !args.bind.ip().is_loopback() {
        bail!("error public-bind-requires --unsafe-public-bind");
    }
    let pool = open_db(&args.data).await?;
    let state = ServerState { pool };
    let app = Router::new()
        .route("/sync", post(sync_handler))
        .with_state(state);
    let listener = TcpListener::bind(args.bind).await?;
    let addr = listener.local_addr()?;
    println!("listening url=http://{}", addr);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn sync_handler(
    State(state): State<ServerState>,
    Json(request): Json<SyncRequest>,
) -> std::result::Result<Json<SyncResponse>, String> {
    let mut conn = state.pool.acquire().await.map_err(|err| err.to_string())?;
    let mut tx = conn.begin().await.map_err(|err| err.to_string())?;
    for change in request.changes {
        let exists = sqlx::query_scalar!(
            r#"SELECT count(*) AS "count!: i64" FROM changes WHERE change_id = ?"#,
            change.change_id
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| err.to_string())?
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
            .map_err(|err| err.to_string())?;
        }
    }
    tx.commit().await.map_err(|err| err.to_string())?;
    let changes = load_server_changes_after(&mut conn, request.after)
        .await
        .map_err(|err| err.to_string())?;
    let cursor = changes
        .iter()
        .filter_map(|change| change.server_seq)
        .max()
        .unwrap_or(request.after);
    Ok(Json(SyncResponse { cursor, changes }))
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
    let summary = run_sync_once(conn, &server).await?;
    println!(
        "synced pushed={} pulled={} cursor={}",
        summary.pushed, summary.pulled, summary.cursor
    );
    Ok(())
}

pub(crate) async fn run_sync_once(
    conn: &mut SqliteConnection,
    server: &str,
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
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?
        .post(url)
        .json(&SyncRequest {
            client_id,
            after,
            changes,
        })
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
