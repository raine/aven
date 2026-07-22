use anyhow::{Result, bail};
use serde_json::json;
use sqlx::{Row, SqliteConnection};
use std::path::Path;

use crate::db::{Database, begin_immediate, insert_change};
use crate::ids::{ProjectId, WorkspaceId, now};
use crate::types::Project;
use crate::workspaces::Workspace;

impl Database {
    pub async fn find_project(
        &self,
        workspace_id: &WorkspaceId,
        input: &str,
    ) -> Result<Option<Project>> {
        let mut conn = self.acquire().await?;
        find_project_in_workspace(&mut conn, workspace_id, input).await
    }

    pub async fn find_project_by_id(
        &self,
        workspace_id: &WorkspaceId,
        project_id: &ProjectId,
    ) -> Result<Option<Project>> {
        let mut conn = self.acquire().await?;
        let row = sqlx::query(
            "SELECT id, workspace_id, key, name, prefix
             FROM projects WHERE workspace_id = ? AND id = ? AND deleted = 0",
        )
        .bind(workspace_id)
        .bind(project_id)
        .fetch_optional(&mut *conn)
        .await?;
        Ok(row.map(project_from_row))
    }

    pub async fn resolve_existing_project(
        &self,
        workspace_id: &WorkspaceId,
        project: &str,
    ) -> Result<Project> {
        let mut conn = self.acquire().await?;
        resolve_existing_project_in_workspace(&mut conn, workspace_id, project).await
    }

    pub async fn list_projects(
        &self,
        workspace_id: &WorkspaceId,
        search: Option<&str>,
    ) -> Result<Vec<Project>> {
        let mut conn = self.acquire().await?;
        list_projects_in_workspace(&mut conn, workspace_id, search).await
    }

    pub async fn inferred_project_key_for_add(
        &self,
        workspace_id: &WorkspaceId,
        config_candidate: Option<&str>,
        cwd: &Path,
        git_root: Option<&Path>,
    ) -> Result<Option<String>> {
        let mut conn = self.acquire().await?;
        if let Some(project) = config_candidate {
            return Ok(Some(normalize_key(project)));
        }
        if let Some(project) =
            project_from_path_mapping_in_workspace(&mut conn, workspace_id, cwd, git_root).await?
        {
            return Ok(Some(project.key));
        }
        Ok(git_root
            .and_then(Path::file_name)
            .map(|name| normalize_key(&name.to_string_lossy())))
    }

    pub async fn inferred_existing_project_key(
        &self,
        workspace_id: &WorkspaceId,
        config_candidate: Option<&str>,
        cwd: &Path,
        git_root: Option<&Path>,
    ) -> Result<Option<String>> {
        let mut conn = self.acquire().await?;
        inferred_existing_project_key_in_workspace(
            &mut conn,
            workspace_id,
            config_candidate,
            cwd,
            git_root,
        )
        .await
    }

    pub async fn remove_project_path(
        &self,
        workspace_id: &WorkspaceId,
        project_id: &ProjectId,
        path: &Path,
    ) -> Result<()> {
        let mut conn = self.acquire().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        sqlx::query(
            "DELETE FROM project_paths WHERE workspace_id = ? AND project_id = ? AND path = ?",
        )
        .bind(workspace_id)
        .bind(project_id)
        .bind(path.display().to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn resolve_or_create_project(
        &self,
        workspace_id: &WorkspaceId,
        project: &str,
    ) -> Result<Project> {
        let mut conn = self.acquire().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let project =
            resolve_or_create_project_in_workspace(&mut tx, workspace_id, project).await?;
        tx.commit().await?;
        Ok(project)
    }

    pub async fn resolve_project_for_stored_value(
        &self,
        workspace_id: &WorkspaceId,
        value: &str,
    ) -> Result<Project> {
        let mut conn = self.acquire().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let project = resolve_project_for_stored_value(&mut tx, workspace_id, value).await?;
        tx.commit().await?;
        Ok(project)
    }
}

async fn inferred_existing_project_key_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    config_candidate: Option<&str>,
    cwd: &Path,
    git_root: Option<&Path>,
) -> Result<Option<String>> {
    if let Some(project) = config_candidate
        && let Some(project) = find_project_in_workspace(conn, workspace_id, project).await?
    {
        return Ok(Some(project.key));
    }

    if let Some(project) =
        project_from_path_mapping_in_workspace(conn, workspace_id, cwd, git_root).await?
    {
        return Ok(Some(project.key));
    }

    let Some(root_name) = git_root.and_then(Path::file_name) else {
        return Ok(None);
    };
    find_project_in_workspace(conn, workspace_id, &root_name.to_string_lossy())
        .await
        .map(|project| project.map(|project| project.key))
}

async fn project_from_path_mapping_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    cwd: &Path,
    git_root: Option<&Path>,
) -> Result<Option<Project>> {
    let root = git_root.unwrap_or(cwd);
    let rows = sqlx::query(
        "SELECT p.id, p.workspace_id, p.key, p.name, p.prefix, pp.path
         FROM project_paths pp
         JOIN projects p ON p.workspace_id = pp.workspace_id AND p.id = pp.project_id
         WHERE pp.workspace_id = ? AND p.deleted = 0
         ORDER BY length(pp.path) DESC",
    )
    .bind(workspace_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut best: Option<(ProjectPathMatch, Project)> = None;
    for row in rows {
        let path: String = row.get("path");
        let project = project_from_row(row);
        if let Some(path_match) = matching_project_path(cwd, root, Path::new(&path))
            && best
                .as_ref()
                .is_none_or(|(best_match, _)| path_match.is_better_than(*best_match))
        {
            best = Some((path_match, project));
        }
    }
    Ok(best.map(|(_, project)| project))
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ProjectPathMatch {
    kind: ProjectPathMatchKind,
    len: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ProjectPathMatchKind {
    Root,
    Cwd,
}

impl ProjectPathMatch {
    fn is_better_than(self, other: Self) -> bool {
        self.kind > other.kind || self.kind == other.kind && self.len > other.len
    }
}

fn matching_project_path(cwd: &Path, root: &Path, path: &Path) -> Option<ProjectPathMatch> {
    let len = path.components().count();
    if cwd.starts_with(path) {
        return Some(ProjectPathMatch {
            kind: ProjectPathMatchKind::Cwd,
            len,
        });
    }
    if root == path || root.starts_with(path) {
        return Some(ProjectPathMatch {
            kind: ProjectPathMatchKind::Root,
            len,
        });
    }
    None
}

pub fn normalize_key(input: &str) -> String {
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

pub(crate) async fn find_project_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    input: &str,
) -> Result<Option<Project>> {
    let key = normalize_key(input);
    let row = sqlx::query(
        "SELECT id, workspace_id, key, name, prefix
         FROM projects
         WHERE workspace_id = ? AND deleted = 0 AND (key = ? OR lower(name) = lower(?))",
    )
    .bind(workspace_id)
    .bind(key)
    .bind(input)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.map(project_from_row))
}

pub(crate) async fn resolve_existing_project_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    project: &str,
) -> Result<Project> {
    find_project_in_workspace(conn, workspace_id, project)
        .await?
        .ok_or_else(|| anyhow::anyhow!("error unknown-project input={project}"))
}

#[allow(dead_code)]
pub(crate) async fn create_project(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    name: &str,
) -> Result<Project> {
    create_project_in_workspace(conn, &workspace.id, name)
        .await
        .map(|outcome| outcome.project)
}

#[derive(Debug, Clone)]
pub struct ProjectCreateOutcome {
    pub project: Project,
    pub created: bool,
    pub change_id: Option<String>,
}

pub(crate) async fn create_project_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    name: &str,
) -> Result<ProjectCreateOutcome> {
    let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
    let key = normalize_key(name);
    if key.is_empty() {
        bail!("error invalid-project input={name:?}");
    }
    if let Some(project) = find_project_in_workspace(conn, &workspace.id, &key).await? {
        return Ok(ProjectCreateOutcome {
            project,
            created: false,
            change_id: None,
        });
    }
    if let Some((project, change_id)) =
        restore_deleted_project(conn, &workspace, &key, name).await?
    {
        return Ok(ProjectCreateOutcome {
            project,
            created: true,
            change_id: Some(change_id),
        });
    }
    let prefix = unique_project_prefix(conn, &workspace.id, &key).await?;
    let id = ProjectId::new();
    let ts = now();
    sqlx::query(
        "INSERT INTO projects(id, workspace_id, key, name, prefix, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&workspace.id)
    .bind(&key)
    .bind(name)
    .bind(&prefix)
    .bind(&ts)
    .bind(&ts)
    .execute(&mut *conn)
    .await?;
    let change_id = insert_change(
        conn,
        "project",
        id.as_str(),
        None,
        "create_project",
        json!({
            "workspace_id": &workspace.id,
            "workspace_key": &workspace.key,
            "key": key,
            "name": name,
            "prefix": prefix,
            "created_at": ts
        }),
        None,
    )
    .await?;
    Ok(ProjectCreateOutcome {
        project: Project {
            id,
            workspace_id: workspace.id,
            key,
            name: name.to_string(),
            prefix,
        },
        created: true,
        change_id: Some(change_id),
    })
}

async fn restore_deleted_project(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    key: &str,
    name: &str,
) -> Result<Option<(Project, String)>> {
    let Some(row) = sqlx::query(
        "SELECT id, workspace_id, key, prefix
         FROM projects
         WHERE workspace_id = ? AND key = ? AND deleted = 1",
    )
    .bind(&workspace.id)
    .bind(key)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    let id: ProjectId = row.get("id");
    let workspace_id: WorkspaceId = row.get("workspace_id");
    let key: String = row.get("key");
    let prefix: String = row.get("prefix");
    let prefix = if prefix == id.as_str() {
        unique_project_prefix(conn, &workspace_id, &key).await?
    } else {
        prefix
    };
    let ts = now();
    sqlx::query(
        "UPDATE projects SET name = ?, prefix = ?, updated_at = ?, deleted = 0
         WHERE workspace_id = ? AND id = ?",
    )
    .bind(name)
    .bind(&prefix)
    .bind(&ts)
    .bind(&workspace_id)
    .bind(&id)
    .execute(&mut *conn)
    .await?;
    let change_id = insert_change(
        conn,
        "project",
        id.as_str(),
        None,
        "create_project",
        json!({
            "workspace_id": &workspace.id,
            "workspace_key": &workspace.key,
            "key": &key,
            "name": name,
            "prefix": &prefix,
            "created_at": ts
        }),
        None,
    )
    .await?;
    Ok(Some((
        Project {
            id,
            workspace_id,
            key,
            name: name.to_string(),
            prefix,
        },
        change_id,
    )))
}

async fn unique_project_prefix(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    key: &str,
) -> Result<String> {
    let base = prefix_base(key);
    let mut candidate = base.clone();
    let mut n = 2;
    while sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM projects WHERE workspace_id = ? AND prefix = ?",
    )
    .bind(workspace_id)
    .bind(&candidate)
    .fetch_one(&mut *conn)
    .await?
        > 0
    {
        candidate = format!("{}{}", base.chars().take(2).collect::<String>(), n);
        n += 1;
    }
    Ok(candidate)
}

pub fn prefix_base(key: &str) -> String {
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

pub(crate) async fn list_projects_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    search: Option<&str>,
) -> Result<Vec<Project>> {
    let search = search.map(normalize_key);
    let rows = sqlx::query(
        "SELECT id, workspace_id, key, name, prefix
         FROM projects
         WHERE workspace_id = ? AND deleted = 0
         ORDER BY key",
    )
    .bind(workspace_id)
    .fetch_all(&mut *conn)
    .await?;
    let projects = rows.into_iter().map(project_from_row).collect::<Vec<_>>();
    Ok(projects
        .into_iter()
        .filter(|project| {
            search.as_deref().is_none_or(|search| {
                project.key.contains(search) || project.name.to_lowercase().contains(search)
            })
        })
        .collect())
}

pub(crate) async fn resolve_or_create_project_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    project: &str,
) -> Result<Project> {
    if let Some(existing) = find_project_in_workspace(conn, workspace_id, project).await? {
        return Ok(existing);
    }
    create_project_in_workspace(conn, workspace_id, project)
        .await
        .map(|outcome| outcome.project)
}

pub(crate) async fn resolve_project_for_stored_value(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    value: &str,
) -> Result<Project> {
    if let Ok(project_id) = value.parse::<ProjectId>()
        && let Some(row) = sqlx::query(
            "SELECT id, workspace_id, key, name, prefix
             FROM projects WHERE workspace_id = ? AND id = ? AND deleted = 0",
        )
        .bind(workspace_id)
        .bind(&project_id)
        .fetch_optional(&mut *conn)
        .await?
    {
        return Ok(project_from_row(row));
    }
    resolve_or_create_project_in_workspace(conn, workspace_id, value).await
}

pub(crate) fn project_from_row(row: sqlx::sqlite::SqliteRow) -> Project {
    Project {
        id: row.get("id"),
        workspace_id: row.get("workspace_id"),
        key: row.get("key"),
        name: row.get("name"),
        prefix: row.get("prefix"),
    }
}
