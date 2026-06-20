use anyhow::{Result, bail};
use sqlx::SqliteConnection;

use crate::fuzzy::is_near;
use crate::projects::normalize_key;
use crate::workspaces::active_workspace_id;

pub(crate) fn normalize_label(input: &str) -> String {
    normalize_key(input)
}

async fn near_labels(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    input: &str,
) -> Result<Vec<String>> {
    let needle = normalize_label(input);
    Ok(list_labels_in_workspace(conn, workspace_id, None)
        .await?
        .into_iter()
        .filter(|label| is_near(&needle, label))
        .collect())
}

pub(crate) async fn list_labels(
    conn: &mut SqliteConnection,
    search: Option<&str>,
) -> Result<Vec<String>> {
    list_labels_in_workspace(conn, active_workspace_id().as_str(), search).await
}

pub(crate) async fn list_labels_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    search: Option<&str>,
) -> Result<Vec<String>> {
    let search = search.map(normalize_label);
    let labels = sqlx::query_scalar::<_, String>(
        "SELECT name FROM labels WHERE workspace_id = ? ORDER BY name",
    )
    .bind(workspace_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(labels
        .into_iter()
        .filter(|label| search.as_deref().is_none_or(|search| label.contains(search)))
        .collect())
}

#[allow(dead_code)]
pub(crate) async fn ensure_label_exists(
    conn: &mut SqliteConnection,
    label: &str,
) -> Result<String> {
    ensure_label_exists_in_workspace(conn, active_workspace_id().as_str(), label).await
}

pub(crate) async fn ensure_label_exists_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    label: &str,
) -> Result<String> {
    let label = normalize_label(label);
    if sqlx::query_scalar::<_, i64>("SELECT count(*) FROM labels WHERE workspace_id = ? AND name = ?")
        .bind(workspace_id)
        .bind(&label)
        .fetch_one(&mut *conn)
        .await?
        > 0
    {
        Ok(label)
    } else {
        let choices = near_labels(conn, workspace_id, &label).await?;
        eprintln!("error unknown-label input={}", label);
        for choice in choices {
            eprintln!("choice {choice}");
        }
        eprintln!("hint \"create the label explicitly\"");
        bail!("unknown label");
    }
}

#[allow(dead_code)]
pub(crate) async fn resolve_labels(
    conn: &mut SqliteConnection,
    labels: &[String],
) -> Result<Vec<String>> {
    resolve_labels_in_workspace(conn, active_workspace_id().as_str(), labels).await
}

pub(crate) async fn resolve_labels_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    labels: &[String],
) -> Result<Vec<String>> {
    let mut resolved = Vec::with_capacity(labels.len());
    for label in labels {
        resolved.push(ensure_label_exists_in_workspace(conn, workspace_id, label).await?);
    }
    Ok(resolved)
}
