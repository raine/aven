use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use anyhow::Result;
use aven_core::db::Database;

use crate::attachments::storage::object_path;
use crate::workspaces::Workspace;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AttachmentSaveError {
    Invalidated,
    BlobUnavailable,
    StorageUnavailable,
    DestinationExists(PathBuf),
    DestinationParentUnavailable(PathBuf),
    WriteFailed(PathBuf),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AttachmentSaveOutcome {
    pub(crate) destination: PathBuf,
    pub(crate) lease_release_failed: bool,
}

pub(crate) async fn save_attachment(
    database: &Database,
    workspace: &Workspace,
    blob_dir: &Path,
    attachment_id: &str,
    destination: PathBuf,
) -> Result<AttachmentSaveOutcome, AttachmentSaveError> {
    let read_lease = database
        .acquire_live_attachment_read_lease(workspace, attachment_id)
        .await
        .map_err(classify_read_error)?;
    let copy_result = match object_path(blob_dir, &read_lease.sha256) {
        Ok(source) => {
            let copy_destination = destination.clone();
            let blocking_failure_destination = destination.clone();
            crate::attachments::blocking::run(move || {
                Ok(copy_without_overwrite(&source, &copy_destination))
            })
            .await
            .unwrap_or(Err(AttachmentSaveError::WriteFailed(
                blocking_failure_destination,
            )))
        }
        Err(_) => Err(AttachmentSaveError::StorageUnavailable),
    };
    let lease_release_failed = database
        .release_attachment_lease(&read_lease.lease_id)
        .await
        .is_err();
    copy_result?;
    Ok(AttachmentSaveOutcome {
        destination,
        lease_release_failed,
    })
}

fn classify_read_error(error: anyhow::Error) -> AttachmentSaveError {
    let message = error.to_string();
    if message.contains("attachment-invalidated") {
        AttachmentSaveError::Invalidated
    } else if message.contains("attachment-blob-unavailable") {
        AttachmentSaveError::BlobUnavailable
    } else {
        AttachmentSaveError::StorageUnavailable
    }
}

fn copy_without_overwrite(source: &Path, destination: &Path) -> Result<(), AttachmentSaveError> {
    if !source.is_file() {
        return Err(AttachmentSaveError::BlobUnavailable);
    }
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(AttachmentSaveError::DestinationParentUnavailable(
            parent.to_path_buf(),
        ));
    }
    let mut output = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
    {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(AttachmentSaveError::DestinationExists(
                destination.to_path_buf(),
            ));
        }
        Err(_) => {
            return Err(AttachmentSaveError::WriteFailed(destination.to_path_buf()));
        }
    };
    let copy_result = fs::File::open(source)
        .and_then(|mut input| io::copy(&mut input, &mut output))
        .and_then(|_| output.sync_all());
    if copy_result.is_err() {
        drop(output);
        let _ = fs::remove_file(destination);
        return Err(AttachmentSaveError::WriteFailed(destination.to_path_buf()));
    }
    Ok(())
}
