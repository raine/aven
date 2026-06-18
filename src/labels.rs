use anyhow::{Result, bail};
use sqlx::SqliteConnection;

use crate::fuzzy::is_near;
use crate::projects::normalize_key;

pub(crate) fn normalize_label(input: &str) -> String {
    normalize_key(input)
}

async fn near_labels(conn: &mut SqliteConnection, input: &str) -> Result<Vec<String>> {
    let needle = normalize_label(input);
    Ok(list_labels(conn, None)
        .await?
        .into_iter()
        .filter(|label| is_near(&needle, label))
        .collect())
}

pub(crate) async fn list_labels(
    conn: &mut SqliteConnection,
    search: Option<&str>,
) -> Result<Vec<String>> {
    let search = search.map(normalize_label);
    let labels = sqlx::query_scalar!(r#"SELECT name AS "name!: String" FROM labels ORDER BY name"#)
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

pub(crate) async fn ensure_label_exists(
    conn: &mut SqliteConnection,
    label: &str,
) -> Result<String> {
    let label = normalize_label(label);
    if sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!: i64" FROM labels WHERE name = ?"#,
        label
    )
    .fetch_one(&mut *conn)
    .await?
        > 0
    {
        Ok(label)
    } else {
        let choices = near_labels(conn, &label).await?;
        eprintln!("error unknown-label input={}", label);
        for choice in choices {
            eprintln!("choice {choice}");
        }
        eprintln!("hint \"create the label explicitly\"");
        bail!("unknown label");
    }
}

pub(crate) async fn resolve_labels(
    conn: &mut SqliteConnection,
    labels: &[String],
) -> Result<Vec<String>> {
    let mut resolved = Vec::with_capacity(labels.len());
    for label in labels {
        resolved.push(ensure_label_exists(conn, label).await?);
    }
    Ok(resolved)
}
