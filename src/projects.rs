use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::json;
use sqlx::SqliteConnection;

use crate::db::insert_change;
use crate::fuzzy::is_near;
use crate::ids::now;
use crate::render::{print_near_error, quote};
use crate::types::Project;

pub(crate) fn normalize_key(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in input.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

pub(crate) async fn resolve_project_for_add(
    conn: &mut SqliteConnection,
    project: Option<&str>,
) -> Result<Project> {
    if let Some(project) = project {
        if let Some(existing) = find_project(conn, project).await? {
            return Ok(existing);
        }
        let choices = near_projects(conn, project).await?;
        if !choices.is_empty() {
            print_near_error("project", project, &choices);
            bail!("near-match project");
        }
        return create_project(conn, project).await;
    }
    if let Some(project) = project_from_path_mapping(conn).await? {
        return Ok(project);
    }
    if let Some(root_name) = git_root_name()? {
        if let Some(existing) = find_project(conn, &root_name).await? {
            return Ok(existing);
        }
        let choices = near_projects(conn, &root_name).await?;
        if !choices.is_empty() {
            print_near_error("project", &root_name, &choices);
            bail!("near-match project");
        }
        return create_project(conn, &root_name).await;
    }
    bail!("error project-required");
}

pub(crate) async fn resolve_existing_project(
    conn: &mut SqliteConnection,
    project: &str,
) -> Result<Project> {
    if let Some(project) = find_project(conn, project).await? {
        return Ok(project);
    }
    let choices = near_projects(conn, project).await?;
    if !choices.is_empty() {
        print_near_error("project", project, &choices);
    } else {
        eprintln!("error unknown-project input={}", project);
    }
    bail!("unknown project");
}

pub(crate) async fn find_project(
    conn: &mut SqliteConnection,
    input: &str,
) -> Result<Option<Project>> {
    let key = normalize_key(input);
    let row = sqlx::query!(
        r#"SELECT key AS "key!: String", name AS "name!: String", prefix AS "prefix!: String"
         FROM projects
         WHERE deleted = 0 AND (key = ? OR lower(name) = lower(?))"#,
        key,
        input,
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.map(|row| Project {
        key: row.key,
        name: row.name,
        prefix: row.prefix,
    }))
}

pub(crate) async fn create_project(conn: &mut SqliteConnection, name: &str) -> Result<Project> {
    let key = normalize_key(name);
    if key.is_empty() {
        bail!("error invalid-project input={}", quote(name));
    }
    if let Some(project) = find_project(conn, &key).await? {
        return Ok(project);
    }
    let prefix = unique_project_prefix(conn, &key).await?;
    let ts = now();
    sqlx::query!(
        "INSERT INTO projects(key, name, prefix, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
        key,
        name,
        prefix,
        ts,
        ts,
    )
    .execute(&mut *conn)
    .await?;
    insert_change(
        conn,
        "project",
        &key,
        None,
        "create_project",
        json!({ "key": key, "name": name, "prefix": prefix, "created_at": ts }),
        None,
    )
    .await?;
    Ok(Project {
        key,
        name: name.to_string(),
        prefix,
    })
}

async fn unique_project_prefix(conn: &mut SqliteConnection, key: &str) -> Result<String> {
    let base = prefix_base(key);
    let mut candidate = base.clone();
    let mut n = 2;
    while sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!: i64" FROM projects WHERE prefix = ?"#,
        candidate
    )
    .fetch_one(&mut *conn)
    .await?
        > 0
    {
        candidate = format!("{}{}", base.chars().take(2).collect::<String>(), n);
        n += 1;
    }
    Ok(candidate)
}

pub(crate) fn prefix_base(key: &str) -> String {
    let words: Vec<&str> = key.split('-').filter(|word| !word.is_empty()).collect();
    if words.len() >= 2 {
        return words
            .iter()
            .filter_map(|word| word.chars().next())
            .take(3)
            .collect::<String>()
            .to_ascii_uppercase();
    }
    let key = words.first().copied().unwrap_or(key);
    let mut out = String::new();
    let mut chars = key.chars();
    if let Some(first) = chars.next() {
        out.push(first);
    }
    for ch in chars {
        if !"aeiou".contains(ch) {
            out.push(ch);
        }
        if out.len() >= 3 {
            break;
        }
    }
    for ch in key.chars() {
        if out.len() >= 3 {
            break;
        }
        if !out.contains(ch) {
            out.push(ch);
        }
    }
    while out.len() < 3 {
        out.push('X');
    }
    out.to_ascii_uppercase()
}

async fn project_from_path_mapping(conn: &mut SqliteConnection) -> Result<Option<Project>> {
    let cwd = fs::canonicalize(env::current_dir()?)?;
    let rows = sqlx::query!(
        r#"SELECT p.key AS "key!: String", p.name AS "name!: String",
         p.prefix AS "prefix!: String", pp.path AS "path!: String"
         FROM project_paths pp JOIN projects p ON p.key = pp.project_key
         ORDER BY length(pp.path) DESC"#,
    )
    .fetch_all(&mut *conn)
    .await?;
    for row in rows {
        let project = Project {
            key: row.key,
            name: row.name,
            prefix: row.prefix,
        };
        if cwd.starts_with(Path::new(&row.path)) {
            return Ok(Some(project));
        }
    }
    Ok(None)
}

fn git_root_name() -> Result<Option<String>> {
    let cwd = fs::canonicalize(env::current_dir()?)?;
    let Some(root) = git_root(&cwd) else {
        return Ok(None);
    };
    Ok(root
        .file_name()
        .map(|name| name.to_string_lossy().to_string()))
}

fn git_root(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
        .map(Path::to_path_buf)
}

pub(crate) async fn add_project_path(
    conn: &mut SqliteConnection,
    project_key: &str,
    path: &Path,
) -> Result<()> {
    let path =
        fs::canonicalize(path).with_context(|| format!("could not resolve {}", path.display()))?;
    let path = path.display().to_string();
    sqlx::query!(
        "INSERT OR IGNORE INTO project_paths(project_key, path) VALUES (?, ?)",
        project_key,
        path,
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn near_projects(conn: &mut SqliteConnection, input: &str) -> Result<Vec<String>> {
    let needle = normalize_key(input);
    let projects = list_projects(conn, None).await?;
    Ok(projects
        .into_iter()
        .filter(|project| is_near(&needle, &project.key))
        .map(|project| {
            format!(
                "{} prefix={} name={}",
                project.key,
                project.prefix,
                quote(&project.name)
            )
        })
        .collect())
}

pub(crate) async fn list_projects(
    conn: &mut SqliteConnection,
    search: Option<&str>,
) -> Result<Vec<Project>> {
    let search = search.map(normalize_key);
    let rows = sqlx::query!(
        r#"SELECT key AS "key!: String", name AS "name!: String", prefix AS "prefix!: String"
         FROM projects
         WHERE deleted = 0
         ORDER BY key"#,
    )
    .fetch_all(&mut *conn)
    .await?;
    let projects = rows
        .into_iter()
        .map(|row| Project {
            key: row.key,
            name: row.name,
            prefix: row.prefix,
        })
        .collect::<Vec<_>>();
    Ok(projects
        .into_iter()
        .filter(|project| {
            search.as_deref().is_none_or(|search| {
                project.key.contains(search) || project.name.to_lowercase().contains(search)
            })
        })
        .collect())
}
