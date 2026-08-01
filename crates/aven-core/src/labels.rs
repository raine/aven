use crate::ids::WorkspaceId;
use anyhow::{Result, bail};
use sqlx::{Row, SqliteConnection};

use crate::db::Database;
use crate::matching::is_near;
use crate::projects::normalize_key;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelUsage {
    pub name: String,
    pub task_count: usize,
    pub series_count: usize,
}

impl Database {
    pub async fn label_usage(&self, workspace_id: &WorkspaceId) -> Result<Vec<LabelUsage>> {
        let mut conn = self.acquire().await?;
        let rows = sqlx::query(
            "SELECT l.name,
                    (SELECT count(*) FROM task_labels tl
                     WHERE tl.workspace_id = l.workspace_id AND tl.label = l.name) AS task_count,
                    (SELECT count(*) FROM recurrence_series_labels sl
                     WHERE sl.workspace_id = l.workspace_id AND sl.label = l.name) AS series_count
             FROM labels l
             WHERE l.workspace_id = ?
             ORDER BY l.name",
        )
        .bind(workspace_id)
        .fetch_all(&mut *conn)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| LabelUsage {
                name: row.get("name"),
                task_count: usize::try_from(row.get::<i64, _>("task_count")).unwrap_or(usize::MAX),
                series_count: usize::try_from(row.get::<i64, _>("series_count"))
                    .unwrap_or(usize::MAX),
            })
            .collect())
    }

    pub async fn list_labels(
        &self,
        workspace_id: &WorkspaceId,
        search: Option<&str>,
    ) -> Result<Vec<String>> {
        let mut conn = self.acquire().await?;
        list_labels_in_workspace(&mut conn, workspace_id, search).await
    }

    pub async fn resolve_labels(
        &self,
        workspace_id: &WorkspaceId,
        labels: &[String],
    ) -> Result<Vec<String>> {
        let mut conn = self.acquire().await?;
        resolve_labels_in_workspace(&mut conn, workspace_id, labels).await
    }
}

pub fn normalize_label(input: &str) -> String {
    normalize_key(input)
}

async fn near_labels(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    input: &str,
) -> Result<Vec<String>> {
    let needle = normalize_label(input);
    Ok(list_labels_in_workspace(conn, workspace_id, None)
        .await?
        .into_iter()
        .filter(|label| is_near(&needle, label))
        .collect())
}

pub(crate) async fn list_labels_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
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
        .filter(|label| {
            search
                .as_deref()
                .is_none_or(|search| label.contains(search))
        })
        .collect())
}

pub(crate) async fn ensure_label_exists_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    label: &str,
) -> Result<String> {
    let label = normalize_label(label);
    if sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM labels WHERE workspace_id = ? AND name = ?",
    )
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

pub(crate) async fn resolve_labels_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    labels: &[String],
) -> Result<Vec<String>> {
    let mut resolved = Vec::with_capacity(labels.len());
    for label in labels {
        resolved.push(ensure_label_exists_in_workspace(conn, workspace_id, label).await?);
    }
    Ok(resolved)
}
