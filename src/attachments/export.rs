use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sqlx::{Row, SqlitePool};

use crate::attachments::lifecycle::{SystemClock, acquire_lease, release_lease};
use crate::attachments::storage::object_path;
use crate::workspaces::Workspace;

struct LeaseGuard {
    pool: SqlitePool,
    lease_id: Option<String>,
}

impl LeaseGuard {
    async fn release(&mut self) -> Result<()> {
        let Some(lease_id) = self.lease_id.take() else {
            return Ok(());
        };
        let mut conn = self.pool.acquire().await?;
        release_lease(&mut conn, &lease_id).await
    }
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        let Some(lease_id) = self.lease_id.take() else {
            return;
        };
        let pool = self.pool.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Ok(mut conn) = pool.acquire().await {
                    let _ = release_lease(&mut conn, &lease_id).await;
                }
            });
        }
    }
}

pub(crate) struct LeasedImageExport {
    directory: Option<tempfile::TempDir>,
    path: PathBuf,
    lease: LeaseGuard,
}

impl LeasedImageExport {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn into_directory(mut self) -> tempfile::TempDir {
        self.directory.take().expect("image export directory")
    }

    pub(crate) async fn release(&mut self) -> Result<()> {
        self.lease.release().await
    }
}

pub(crate) async fn lease_image_export(
    pool: &SqlitePool,
    workspace: &Workspace,
    blob_dir: &Path,
    attachment_id: &str,
) -> Result<LeasedImageExport> {
    let mut conn = pool.acquire().await?;
    let row = sqlx::query(
        "SELECT ta.sha256, ta.media_type, bi.available
         FROM task_attachments ta
         JOIN tasks t ON t.workspace_id = ta.workspace_id AND t.id = ta.task_id
         LEFT JOIN blob_inventory bi ON bi.sha256 = ta.sha256
         WHERE ta.workspace_id = ? AND ta.attachment_id = ?
           AND ta.deleted = 0 AND t.deleted = 0",
    )
    .bind(&workspace.id)
    .bind(attachment_id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| anyhow::anyhow!("error attachment-invalidated"))?;

    let sha256: String = row.get("sha256");
    let media_type: String = row.get("media_type");
    let extension = image_extension(&media_type)?;
    let available = row.try_get::<bool, _>("available").unwrap_or(false);
    if !available {
        bail!("error attachment-blob-unavailable");
    }

    let lease_id = acquire_lease(&mut conn, &sha256, "read", &SystemClock).await?;
    drop(conn);
    let lease = LeaseGuard {
        pool: pool.clone(),
        lease_id: Some(lease_id),
    };
    let (directory, path) = create_image_export(blob_dir, &sha256, extension).await?;
    Ok(LeasedImageExport {
        directory: Some(directory),
        path,
        lease,
    })
}

async fn create_image_export(
    blob_dir: &Path,
    sha256: &str,
    extension: &str,
) -> Result<(tempfile::TempDir, PathBuf)> {
    let source = object_path(blob_dir, sha256)?;
    let extension = extension.to_string();
    crate::attachments::blocking::run(move || {
        if !source.is_file() {
            bail!("error attachment-blob-unavailable");
        }
        let directory = tempfile::Builder::new()
            .prefix("aven-image-view-")
            .tempdir()
            .context("could not create image viewer export")?;
        let path = directory.path().join(format!("attachment.{extension}"));
        fs::copy(source, &path).context("could not export attachment for image viewer")?;
        Ok((directory, path))
    })
    .await
}

fn image_extension(media_type: &str) -> Result<&'static str> {
    match media_type {
        "image/png" => Ok("png"),
        "image/jpeg" => Ok("jpg"),
        "image/gif" => Ok("gif"),
        "image/webp" => Ok("webp"),
        _ => bail!("error attachment-format-unsupported"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_safe_image_extensions() {
        assert_eq!(image_extension("image/png").unwrap(), "png");
        assert_eq!(image_extension("image/jpeg").unwrap(), "jpg");
        assert_eq!(image_extension("image/gif").unwrap(), "gif");
        assert_eq!(image_extension("image/webp").unwrap(), "webp");
        assert!(image_extension("image/svg+xml").is_err());
    }
}
