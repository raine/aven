use std::fs;
use std::path::Path;

use anyhow::{Result, bail};
use serde::Serialize;

use aven_core::db::Database;

use crate::attachments::optimization::ImageOptimizationPolicy;
use crate::attachments::storage::object_path;
use crate::cli::{
    AttachmentAddArgs, AttachmentCommand, AttachmentDeleteArgs, AttachmentGetArgs,
    AttachmentListArgs, AttachmentPruneArgs, AttachmentSubcommand,
};
use crate::config::{self as app_config, AppConfig};
use crate::operations::AttachmentAddInput;
use crate::render::print_json_pretty;
use crate::types::TaskAttachment;
use crate::workspaces::Workspace;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    optimized: Option<bool>,
}

fn attachment_json_item(
    att: &TaskAttachment,
    has_blob: bool,
    optimized: Option<bool>,
) -> AttachmentJsonItem {
    AttachmentJsonItem {
        attachment_id: att.attachment_id.clone(),
        task_id: att.task_id.to_string(),
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
        optimized,
    }
}

async fn attachment_task_ref(
    database: &Database,
    workspace: &Workspace,
    attachment: &TaskAttachment,
) -> Result<String> {
    let task = database
        .resolve_task_ref(workspace, attachment.task_id.as_str())
        .await?;
    Ok(database
        .display_ref_context(&workspace.id)
        .await?
        .display_ref(&task))
}

pub(crate) async fn cmd_attachment(
    database: &Database,
    workspace: &Workspace,
    config: &AppConfig,
    db_path: &Path,
    args: AttachmentCommand,
) -> Result<()> {
    match args.command {
        AttachmentSubcommand::Add(args) => {
            cmd_attachment_add(database, workspace, config, db_path, args).await
        }
        AttachmentSubcommand::List(args) => cmd_attachment_list(database, workspace, args).await,
        AttachmentSubcommand::Get(args) => {
            cmd_attachment_get(database, workspace, config, db_path, args).await
        }
        AttachmentSubcommand::Delete(args) => {
            cmd_attachment_delete(database, workspace, config, args).await
        }
        AttachmentSubcommand::Prune(args) => {
            cmd_attachment_prune(database, config, db_path, args).await
        }
    }
}

pub(crate) async fn cmd_attachment_add(
    database: &Database,
    workspace: &Workspace,
    config: &AppConfig,
    db_path: &Path,
    args: AttachmentAddArgs,
) -> Result<()> {
    let task = database.resolve_task_ref(workspace, &args.task_ref).await?;
    let declared_media_type = args.media_type;
    let filename = args
        .filename
        .or_else(|| Some(default_attachment_filename(&args.path)));
    let bytes = fs::read(&args.path)?;
    let blob_dir = app_config::resolve_blob_dir(db_path, config)?;
    let optimization_policy = if args.optimize {
        ImageOptimizationPolicy::Optimize
    } else if args.no_optimize || !config.local.image_optimization.optimizes_file_attachments() {
        ImageOptimizationPolicy::Preserve
    } else {
        ImageOptimizationPolicy::Optimize
    };

    let add_outcome = database
        .add_task_attachment(
            workspace,
            &blob_dir,
            config.local.attachment_lifecycle.policy(),
            &task.id,
            AttachmentAddInput {
                filename,
                alt_text: args.alt,
                declared_media_type,
                bytes,
                optimization_policy,
                dedupe_existing: false,
            },
        )
        .await?;
    let outcome = &add_outcome.outcome;

    if args.json {
        let item = attachment_json_item(
            &outcome.attachment,
            outcome.has_blob,
            Some(add_outcome.optimized),
        );
        print_json_pretty(&item)?;
    } else {
        let ref_str = database
            .display_ref_context(&workspace.id)
            .await?
            .display_ref(&task);
        println!(
            "attachment-added {} attachment_id={} media_type={} byte_size={} has_blob={} optimized={}",
            ref_str,
            outcome.attachment.attachment_id,
            outcome.attachment.media_type,
            outcome.attachment.byte_size,
            outcome.has_blob,
            add_outcome.optimized,
        );
    }
    Ok(())
}

pub(crate) async fn cmd_attachment_list(
    database: &Database,
    workspace: &Workspace,
    args: AttachmentListArgs,
) -> Result<()> {
    let task = database.resolve_task_ref(workspace, &args.task_ref).await?;
    let items = database
        .attachment_read_items_by_task(&workspace.id, &task.id, args.all)
        .await?;

    if args.json {
        let attachments = items
            .iter()
            .map(|item| attachment_json_item(&item.attachment, item.has_blob, None))
            .collect::<Vec<_>>();
        print_json_pretty(&attachments)?;
    } else {
        for item in &items {
            let att = &item.attachment;
            println!(
                "attachment attachment_id={} media_type={} byte_size={} deleted={}",
                att.attachment_id,
                att.media_type,
                att.byte_size,
                if att.deleted { "yes" } else { "no" },
            );
        }
    }
    Ok(())
}

pub(crate) async fn cmd_attachment_get(
    database: &Database,
    workspace: &Workspace,
    config: &AppConfig,
    db_path: &Path,
    args: AttachmentGetArgs,
) -> Result<()> {
    let outcome = database
        .attachment_by_id(workspace, &args.attachment_id)
        .await?;

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
        let lease_id = database
            .acquire_attachment_lease(&outcome.attachment.sha256, "read")
            .await?;
        let obj_path = object_path(&blob_dir, &outcome.attachment.sha256)?;
        let copy_result = fs::copy(&obj_path, output_path);
        database.release_attachment_lease(&lease_id).await?;
        copy_result?;
    }

    if args.json {
        let item = attachment_json_item(&outcome.attachment, outcome.has_blob, None);
        print_json_pretty(&item)?;
    } else if let Some(ref output_path) = args.output {
        let ref_str = attachment_task_ref(database, workspace, &outcome.attachment).await?;
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
        let ref_str = attachment_task_ref(database, workspace, &outcome.attachment).await?;
        println!(
            "attachment attachment_id={} task={} media_type={} byte_size={} has_blob={} deleted={}",
            outcome.attachment.attachment_id,
            ref_str,
            outcome.attachment.media_type,
            outcome.attachment.byte_size,
            outcome.has_blob,
            if outcome.attachment.deleted {
                "yes"
            } else {
                "no"
            },
        );
    }
    Ok(())
}

pub(crate) async fn cmd_attachment_delete(
    database: &Database,
    workspace: &Workspace,
    _config: &AppConfig,
    args: AttachmentDeleteArgs,
) -> Result<()> {
    let outcome = database
        .delete_task_attachment(workspace, &args.attachment_id)
        .await?;

    if args.json {
        let item = attachment_json_item(&outcome.attachment, outcome.has_blob, None);
        print_json_pretty(&item)?;
    } else {
        let ref_str = attachment_task_ref(database, workspace, &outcome.attachment).await?;
        println!(
            "attachment-deleted {} attachment_id={} deleted={}",
            ref_str,
            outcome.attachment.attachment_id,
            if outcome.attachment.deleted {
                "yes"
            } else {
                "no"
            },
        );
    }
    Ok(())
}

#[derive(Serialize)]
struct AttachmentPruneReport {
    mode: &'static str,
    eligible_count: u64,
    eligible_bytes: u64,
    pruned_count: u64,
    pruned_bytes: u64,
}

pub(crate) async fn cmd_attachment_prune(
    database: &Database,
    config: &AppConfig,
    db_path: &Path,
    args: AttachmentPruneArgs,
) -> Result<()> {
    let blob_dir = app_config::resolve_blob_dir(db_path, config)?;
    let summary = database
        .prune_attachments(
            &blob_dir,
            config.local.attachment_lifecycle.policy(),
            args.apply,
        )
        .await?;
    let report = AttachmentPruneReport {
        mode: if args.apply { "apply" } else { "dry-run" },
        eligible_count: summary.eligible.count,
        eligible_bytes: summary.eligible.bytes,
        pruned_count: summary.pruned.count,
        pruned_bytes: summary.pruned.bytes,
    };
    if args.json {
        print_json_pretty(&report)?;
    } else {
        println!(
            "attachment-prune mode={} eligible_count={} eligible_bytes={} pruned_count={} pruned_bytes={}",
            report.mode,
            report.eligible_count,
            report.eligible_bytes,
            report.pruned_count,
            report.pruned_bytes,
        );
    }
    Ok(())
}
