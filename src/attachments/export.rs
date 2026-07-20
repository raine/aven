use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use aven_core::db::Database;

use crate::attachments::storage::object_path;
use crate::workspaces::Workspace;

struct LeaseGuard {
    database: Database,
    lease_id: Option<String>,
}

impl LeaseGuard {
    async fn release(&mut self) -> Result<()> {
        let Some(lease_id) = self.lease_id.take() else {
            return Ok(());
        };
        self.database.release_attachment_lease(&lease_id).await
    }
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        let Some(lease_id) = self.lease_id.take() else {
            return;
        };
        let database = self.database.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = database.release_attachment_lease(&lease_id).await;
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
    database: &Database,
    workspace: &Workspace,
    blob_dir: &Path,
    attachment_id: &str,
) -> Result<LeasedImageExport> {
    let read_lease = database
        .acquire_live_attachment_read_lease(workspace, attachment_id)
        .await?;
    let lease = LeaseGuard {
        database: database.clone(),
        lease_id: Some(read_lease.lease_id),
    };
    let extension = image_extension(&read_lease.media_type)?;
    let (directory, path) = create_image_export(blob_dir, &read_lease.sha256, extension).await?;
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
