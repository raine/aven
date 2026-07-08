use std::fs;
use std::path::Path;

use anyhow::{Result, bail};
use serde::Serialize;
use sqlx::SqliteConnection;

use crate::attachments::storage::object_path;
use crate::cli::{
    AttachmentAddArgs, AttachmentCommand, AttachmentDeleteArgs, AttachmentGetArgs,
    AttachmentListArgs, AttachmentSubcommand,
};
use crate::config::{self as app_config, AppConfig};
use crate::operations::{
    AttachmentAddInput, add_task_attachment, attachment_by_id, attachments_by_task,
    delete_task_attachment,
};
use crate::refs::{DisplayRefContext, resolve_task_ref_in_workspace};
use crate::render::print_json_pretty;
use crate::types::TaskAttachment;
use crate::workspaces::Workspace;

const EXT_MEDIA_TYPES: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
];

fn infer_attachment_media_type(path: &Path) -> Result<String> {
    let ext = path
        .extension()
        .and_then(|v| v.to_str())
        .map(str::to_ascii_lowercase);
    let Some(ext) = ext else {
        bail!("error attachment-media-type-required hint=\"pass --media-type\"");
    };
    for (key, mime) in EXT_MEDIA_TYPES {
        if *key == ext {
            return Ok(mime.to_string());
        }
    }
    bail!("error attachment-media-type-required hint=\"pass --media-type\"");
}

fn default_attachment_filename(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("attachment")
        .to_string()
}

#[derive(Serialize)]
struct AttachmentJsonItem {
    attachment_id: String,
    task_id: String,
    sha256: String,
    byte_size: i64,
    media_type: String,
    filename: Option<String>,
    alt_text: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    created_at: String,
    deleted: bool,
    deleted_at: Option<String>,
    has_blob: bool,
}

fn attachment_json_item(att: &TaskAttachment, has_blob: bool) -> AttachmentJsonItem {
    AttachmentJsonItem {
        attachment_id: att.attachment_id.clone(),
        task_id: att.task_id.clone(),
        sha256: att.sha256.clone(),
        byte_size: att.byte_size,
        media_type: att.media_type.clone(),
        filename: att.filename.clone(),
        alt_text: att.alt_text.clone(),
        width: att.width,
        height: att.height,
        created_at: att.created_at.clone(),
        deleted: att.deleted,
        deleted_at: att.deleted_at.clone(),
        has_blob,
    }
}

fn ensure_sync_not_enabled(config: &AppConfig) -> Result<()> {
    if config.sync.enabled {
        bail!(
            "error attachment-sync-enabled hint=\"sync must be disabled for local attachment operations\""
        );
    }
    Ok(())
}

pub(crate) async fn cmd_attachment(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    config: &AppConfig,
    db_path: &Path,
    args: AttachmentCommand,
) -> Result<()> {
    match args.command {
        AttachmentSubcommand::Add(args) => {
            cmd_attachment_add(conn, workspace, config, db_path, args).await
        }
        AttachmentSubcommand::List(args) => cmd_attachment_list(conn, workspace, args).await,
        AttachmentSubcommand::Get(args) => {
            cmd_attachment_get(conn, workspace, config, db_path, args).await
        }
        AttachmentSubcommand::Delete(args) => {
            cmd_attachment_delete(conn, workspace, config, args).await
        }
    }
}

pub(crate) async fn cmd_attachment_add(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    config: &AppConfig,
    db_path: &Path,
    args: AttachmentAddArgs,
) -> Result<()> {
    ensure_sync_not_enabled(config)?;

    let task = resolve_task_ref_in_workspace(conn, workspace, &args.task_ref).await?;
    let media_type = match args.media_type {
        Some(ref mt) => mt.clone(),
        None => infer_attachment_media_type(&args.path)?,
    };
    let filename = args
        .filename
        .or_else(|| Some(default_attachment_filename(&args.path)));
    let bytes = fs::read(&args.path)?;
    let blob_dir = app_config::resolve_blob_dir(db_path, config)?;

    let outcome = add_task_attachment(
        conn,
        workspace,
        &blob_dir,
        &task.id,
        AttachmentAddInput {
            filename,
            alt_text: args.alt,
            media_type,
            width: args.width,
            height: args.height,
            bytes,
        },
    )
    .await?;

    if args.json {
        let item = attachment_json_item(&outcome.attachment, outcome.has_blob);
        print_json_pretty(&item)?;
    } else {
        let ref_str = DisplayRefContext::for_workspace(conn, &workspace.id)
            .await?
            .display_ref(&outcome.task);
        println!(
            "attachment-added {} attachment_id={} media_type={} byte_size={} sha256={} has_blob={}",
            ref_str,
            outcome.attachment.attachment_id,
            outcome.attachment.media_type,
            outcome.attachment.byte_size,
            outcome.attachment.sha256,
            outcome.has_blob,
        );
    }
    Ok(())
}

pub(crate) async fn cmd_attachment_list(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    args: AttachmentListArgs,
) -> Result<()> {
    let task = resolve_task_ref_in_workspace(conn, workspace, &args.task_ref).await?;
    let attachments =
        attachments_by_task(conn, workspace.id.as_str(), task.id.as_str(), args.all).await?;

    if args.json {
        let mut items = Vec::with_capacity(attachments.len());
        for att in &attachments {
            let has_blob = crate::attachments::storage::blob_inventory_row(conn, &att.sha256)
                .await?
                .map(|row| row.available)
                .unwrap_or(false);
            items.push(attachment_json_item(att, has_blob));
        }
        print_json_pretty(&items)?;
    } else {
        for att in &attachments {
            println!(
                "attachment attachment_id={} media_type={} byte_size={} sha256={} deleted={}{}",
                att.attachment_id,
                att.media_type,
                att.byte_size,
                att.sha256,
                if att.deleted { "yes" } else { "no" },
                att.alt_text
                    .as_ref()
                    .map(|a| format!(" alt_text={}", a))
                    .unwrap_or_default(),
            );
        }
    }
    Ok(())
}

pub(crate) async fn cmd_attachment_get(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    config: &AppConfig,
    db_path: &Path,
    args: AttachmentGetArgs,
) -> Result<()> {
    let outcome = attachment_by_id(conn, workspace, &args.attachment_id).await?;

    if !args.all && outcome.attachment.deleted {
        bail!(
            "error attachment-deleted id={} hint=\"pass --all to include deleted attachments\"",
            args.attachment_id
        );
    }

    if let Some(ref output_path) = args.output {
        if output_path.exists() {
            bail!("error output-exists path={}", output_path.display());
        }
        if !outcome.has_blob {
            bail!(
                "error attachment-blob-unavailable attachment_id={}",
                outcome.attachment.attachment_id
            );
        }
        let blob_dir = app_config::resolve_blob_dir(db_path, config)?;
        let obj_path = object_path(&blob_dir, &outcome.attachment.sha256)?;
        fs::copy(&obj_path, output_path)?;
    }

    if args.json {
        let item = attachment_json_item(&outcome.attachment, outcome.has_blob);
        // Show output path in json if requested
        print_json_pretty(&item)?;
    } else if let Some(ref output_path) = args.output {
        let ref_str = DisplayRefContext::for_workspace(conn, &workspace.id)
            .await?
            .display_ref(&outcome.task);
        println!(
            "attachment-get attachment_id={} task={} media_type={} byte_size={} has_blob={} output={}",
            outcome.attachment.attachment_id,
            ref_str,
            outcome.attachment.media_type,
            outcome.attachment.byte_size,
            outcome.has_blob,
            output_path.display(),
        );
    } else {
        let ref_str = DisplayRefContext::for_workspace(conn, &workspace.id)
            .await?
            .display_ref(&outcome.task);
        println!(
            "attachment attachment_id={} task={} media_type={} byte_size={} sha256={} has_blob={} deleted={}{}",
            outcome.attachment.attachment_id,
            ref_str,
            outcome.attachment.media_type,
            outcome.attachment.byte_size,
            outcome.attachment.sha256,
            outcome.has_blob,
            if outcome.attachment.deleted {
                "yes"
            } else {
                "no"
            },
            outcome
                .attachment
                .alt_text
                .as_ref()
                .map(|a| format!(" alt_text={}", a))
                .unwrap_or_default(),
        );
    }
    Ok(())
}

pub(crate) async fn cmd_attachment_delete(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    config: &AppConfig,
    args: AttachmentDeleteArgs,
) -> Result<()> {
    ensure_sync_not_enabled(config)?;

    let outcome = delete_task_attachment(conn, workspace, &args.attachment_id).await?;

    if args.json {
        let item = attachment_json_item(&outcome.attachment, outcome.has_blob);
        print_json_pretty(&item)?;
    } else {
        let ref_str = DisplayRefContext::for_workspace(conn, &workspace.id)
            .await?
            .display_ref(&outcome.task);
        println!(
            "attachment-deleted {} attachment_id={} task={} deleted={}",
            ref_str,
            outcome.attachment.attachment_id,
            outcome.attachment.task_id,
            if outcome.attachment.deleted {
                "yes"
            } else {
                "no"
            },
        );
    }
    Ok(())
}
