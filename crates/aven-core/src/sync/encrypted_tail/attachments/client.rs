use super::super::{Accepted, Authority, codec, domain::Projection, hash, valid};
use super::codec::Descriptor;
use crate::{
    db::{self, Database, begin_immediate},
    sync::{LocalSharedStatePackageKey, bootstrap_format, wire::ChangeWire},
};
use anyhow::{Context as _, Result, ensure};
use sqlx::SqliteConnection;
use std::path::Path;

const DOWNLOAD_CURSOR: &str = "e2ee_image_download_after";
const DOWNLOAD_CANDIDATES: &str = "
    FROM local_e2ee_image_references r
    JOIN local_e2ee_image_objects o ON o.object=r.object
    JOIN task_attachments a ON a.workspace_id=r.workspace AND a.attachment_id=r.reference
    JOIN tasks t ON t.workspace_id=a.workspace_id AND t.id=a.task_id
    WHERE a.deleted=0 AND t.deleted=0
      AND (o.verified=0 OR NOT EXISTS(
          SELECT 1 FROM blob_inventory b WHERE b.sha256=o.sha256 AND b.available=1))";

pub(crate) async fn initialize(
    conn: &mut SqliteConnection,
    association: &str,
    generation: i64,
    prefix: i64,
    package: &bootstrap_format::Package,
    key: &LocalSharedStatePackageKey,
) -> Result<()> {
    let index = bootstrap_format::attachment_index(package, key)?;
    initialize_index(
        conn,
        association,
        generation,
        prefix,
        &hash(&package.descriptor),
        index,
        true,
    )
    .await
}

pub(crate) async fn initialize_index(
    conn: &mut SqliteConnection,
    association: &str,
    generation: i64,
    prefix: i64,
    descriptor: &[u8; 32],
    index: bootstrap_format::AttachmentIndex,
    verified: bool,
) -> Result<()> {
    db::set_meta(conn, DOWNLOAD_CURSOR, "").await?;
    db::set_meta(
        conn,
        super::super::client::INITIAL_IMAGE_WATERMARK,
        if verified { "ready" } else { "pending" },
    )
    .await?;
    sqlx::query("INSERT INTO local_e2ee_image_initialization(singleton,association,sync_generation,prefix_count,descriptor) VALUES(1,?,?,?,?)").bind(association).bind(generation).bind(prefix).bind(descriptor.as_slice()).execute(&mut *conn).await?;
    for (image, sha) in index.objects {
        let d = Descriptor {
            vault: index.context.vault_id,
            stream: index.stream,
            generation: index.context.generation_id,
            object: image.id,
            artifact: image.artifact,
        };
        sqlx::query("INSERT INTO local_e2ee_image_objects(object,descriptor,sha256,origin,verified) VALUES(?,?,?,'bootstrap',?)").bind(d.object.as_slice()).bind(d.encode()?).bind(sha).bind(verified).execute(&mut *conn).await?;
    }
    for r in index.references {
        sqlx::query("INSERT INTO local_e2ee_image_references(workspace,reference,parent,object,origin,deleted) VALUES(?,?,?,?,'bootstrap',?)").bind(r.workspace).bind(r.reference).bind(r.task).bind(r.object.map(|id|id.to_vec())).bind(r.deleted).execute(&mut *conn).await?;
    }
    Ok(())
}
pub(crate) async fn validate(
    conn: &mut SqliteConnection,
    association: &str,
    prefix: i64,
    descriptor: &[u8; 32],
) -> Result<()> {
    let ok:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_e2ee_image_initialization WHERE singleton=1 AND association=? AND prefix_count=? AND descriptor=? AND association=(SELECT value FROM meta WHERE key='e2ee_association') AND sync_generation=CAST((SELECT value FROM meta WHERE key='sync_generation') AS INTEGER))").bind(association).bind(prefix).bind(descriptor.as_slice()).fetch_one(&mut *conn).await?;
    ensure!(ok, "error encrypted-image-reinitialization-required");
    super::super::client::initial_image_watermark(conn).await?;
    let missing:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM task_attachments a WHERE NOT EXISTS(SELECT 1 FROM local_e2ee_image_references r WHERE r.workspace=a.workspace_id AND r.reference=a.attachment_id) AND NOT EXISTS(SELECT 1 FROM changes c WHERE c.change_id=a.created_by_change_id AND c.server_seq IS NULL))").fetch_one(conn).await?;
    ensure!(!missing, "error encrypted-image-reinitialization-required");
    Ok(())
}
pub(crate) async fn accept(
    conn: &mut SqliteConnection,
    accepted: &Accepted,
    change: &ChangeWire,
) -> Result<()> {
    let p = codec::parse(&accepted.record)?.projection;
    match p {
        Projection::Ref {
            workspace,
            task,
            reference,
            descriptor,
            ..
        } => {
            let d = Descriptor::decode(&descriptor)?;
            let sha = change.payload["sha256"]
                .as_str()
                .context("error encrypted-image-hash")?;
            let old: Option<(Vec<u8>, String)> = sqlx::query_as(
                "SELECT descriptor,sha256 FROM local_e2ee_image_objects WHERE object=?",
            )
            .bind(d.object.as_slice())
            .fetch_optional(&mut *conn)
            .await?;
            valid(
                old.as_ref()
                    .is_none_or(|(bytes, h)| *bytes == descriptor && h == sha),
            )?;
            sqlx::query("INSERT OR IGNORE INTO local_e2ee_image_objects(object,descriptor,sha256,origin) VALUES(?,?,?,?)").bind(d.object.as_slice()).bind(&descriptor).bind(sha).bind(&change.change_id).execute(&mut *conn).await?;
            let old:Option<(String,Option<Vec<u8>>,String)>=sqlx::query_as("SELECT parent,object,origin FROM local_e2ee_image_references WHERE workspace=? AND reference=?").bind(&workspace).bind(&reference).fetch_optional(&mut *conn).await?;
            valid(old.as_ref().is_none_or(|(p, o, id)| {
                p == &task && o.as_deref() == Some(d.object.as_slice()) && id == &change.change_id
            }))?;
            if old.is_none() {
                sqlx::query("UPDATE local_e2ee_image_objects SET verified=0 WHERE object=?")
                    .bind(d.object.as_slice())
                    .execute(&mut *conn)
                    .await?;
            }
            sqlx::query("INSERT OR IGNORE INTO local_e2ee_image_references(workspace,reference,parent,object,origin,deleted) VALUES(?,?,?,?,?,0)").bind(workspace).bind(reference).bind(task).bind(d.object.as_slice()).bind(&change.change_id).execute(&mut *conn).await?;
        }
        Projection::Unref {
            workspace,
            task,
            reference,
        } => {
            let n=sqlx::query("UPDATE local_e2ee_image_references SET deleted=1 WHERE workspace=? AND reference=? AND parent=?").bind(workspace).bind(reference).bind(task).execute(&mut *conn).await?.rows_affected();
            valid(n == 1)?;
        }
        _ => {}
    }
    sqlx::query("DELETE FROM local_e2ee_image_preparation WHERE operation_id=?")
        .bind(&change.change_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Exact image bytes for one bounded upload. No plaintext hash leaves core.
pub struct Upload {
    pub workspace: String,
    pub descriptor: Vec<u8>,
    pub object: [u8; 32],
    pub commitment: [u8; 32],
    pub records: Vec<Vec<u8>>,
}
impl Upload {
    fn new(change: &ChangeWire, d: &Descriptor, records: Vec<Vec<u8>>) -> Result<Self> {
        let descriptor = d.encode()?;
        Ok(Self {
            workspace: change.payload["workspace_id"]
                .as_str()
                .context("error encrypted-image-workspace")?
                .into(),
            commitment: hash(&descriptor),
            descriptor,
            object: d.object,
            records,
        })
    }
}

/// Pins the source and stages exact ciphertext for a new image head.
pub(in crate::sync::encrypted_tail) async fn stage(
    conn: &mut SqliteConnection,
    a: &Authority,
    c: &ChangeWire,
    blob_dir: &Path,
) -> Result<(Projection, Upload)> {
    let sha = c.payload["sha256"]
        .as_str()
        .context("error encrypted-image-source")?;
    let source = read_source(
        blob_dir,
        sha,
        c.payload["byte_size"]
            .as_u64()
            .context("error encrypted-image-size")?,
    )
    .await?;
    let image = crate::attachments::decode::validate_image(
        source,
        Some(
            c.payload["media_type"]
                .as_str()
                .context("error encrypted-image-media")?
                .into(),
        ),
    )
    .await?;
    valid(
        c.payload["width"].as_i64() == Some(image.facts.width)
            && c.payload["height"].as_i64() == Some(image.facts.height),
    )?;
    let reusable:Option<Vec<u8>>=sqlx::query_scalar("SELECT o.descriptor FROM local_e2ee_image_objects o JOIN local_e2ee_image_references r ON r.object=o.object WHERE o.sha256=? AND r.workspace=? ORDER BY o.object LIMIT 1").bind(sha).bind(c.payload["workspace_id"].as_str()).fetch_optional(&mut *conn).await?;
    let (d, records) = if let Some(bytes) = reusable {
        let d = Descriptor::decode(&bytes)?;
        let records = d.reconstruct(a, &image.bytes, sha)?;
        (d, records)
    } else {
        Descriptor::seal(a, &image.bytes)?
    };
    let descriptor = d.encode()?;
    sqlx::query("INSERT INTO local_e2ee_image_preparation(singleton,operation_id,descriptor,sha256) VALUES(1,?,?,?)").bind(&c.change_id).bind(&descriptor).bind(sha).execute(&mut *conn).await?;
    for (i, record) in records.iter().enumerate() {
        sqlx::query(
            "INSERT INTO local_e2ee_image_staging(operation_id,chunk_index,bytes) VALUES(?,?,?)",
        )
        .bind(&c.change_id)
        .bind(i as i64)
        .bind(record)
        .execute(&mut *conn)
        .await?;
    }
    let (deleted,version):(bool,Option<String>)=sqlx::query_as("SELECT t.deleted,(SELECT version FROM field_versions f WHERE f.workspace_id=t.workspace_id AND f.entity_type='task' AND f.entity_id=t.id AND f.field='deleted') FROM tasks t WHERE t.workspace_id=? AND t.id=?").bind(c.payload["workspace_id"].as_str()).bind(&c.entity_id).fetch_one(&mut *conn).await?;
    let projection = Projection::Ref {
        workspace: c.payload["workspace_id"]
            .as_str()
            .context("error encrypted-image-workspace")?
            .into(),
        task: c.entity_id.clone(),
        reference: c.payload["attachment_id"]
            .as_str()
            .context("error encrypted-image-reference")?
            .into(),
        descriptor,
        deleted,
        version,
    };
    Ok((projection, Upload::new(c, &d, records)?))
}

/// Verifies the frozen Ref's staged ciphertext, rebuilding missing chunks only
/// from the saved recipe and the pinned source.
pub(in crate::sync::encrypted_tail) async fn frozen_upload(
    conn: &mut SqliteConnection,
    a: &Authority,
    c: &ChangeWire,
    record: &[u8],
    blob_dir: &Path,
) -> Result<Upload> {
    let (bytes, saved_sha): (Vec<u8>, String) = sqlx::query_as(
        "SELECT descriptor,sha256 FROM local_e2ee_image_preparation WHERE operation_id=?",
    )
    .bind(&c.change_id)
    .fetch_optional(&mut *conn)
    .await?
    .context("error encrypted-image-preparation-required")?;
    let Projection::Ref { descriptor, .. } = codec::parse(record)?.projection else {
        anyhow::bail!("error encrypted-image-preparation")
    };
    valid(descriptor == bytes && c.payload["sha256"].as_str() == Some(&saved_sha))?;
    let d = Descriptor::decode(&bytes)?;
    let rows: Vec<(i64, Vec<u8>)> = sqlx::query_as("SELECT chunk_index,bytes FROM local_e2ee_image_staging WHERE operation_id=? ORDER BY chunk_index")
        .bind(&c.change_id).fetch_all(&mut *conn).await?;
    for (index, chunk) in &rows {
        d.verify_chunk(usize::try_from(*index)?, chunk)?;
    }
    let records = if rows.len() == d.artifact.chunks.len() {
        let records: Vec<Vec<u8>> = rows.into_iter().map(|(_, b)| b).collect();
        d.verify(&records)?;
        records
    } else {
        let source = read_source(blob_dir, &saved_sha, d.artifact.total).await?;
        let records = d.reconstruct(a, &source, &saved_sha)?;
        for (i, record) in records.iter().enumerate() {
            sqlx::query("INSERT OR IGNORE INTO local_e2ee_image_staging(operation_id,chunk_index,bytes) VALUES(?,?,?)").bind(&c.change_id).bind(i as i64).bind(record).execute(&mut *conn).await?;
        }
        records
    };
    Upload::new(c, &d, records)
}

pub(in crate::sync::encrypted_tail) async fn supersede(
    conn: &mut SqliteConnection,
    a: &Authority,
    change: &ChangeWire,
    mut projection: Projection,
    blob_dir: &Path,
) -> Result<Projection> {
    let Projection::Ref { descriptor, .. } = &mut projection else {
        anyhow::bail!("error encrypted-image-preparation");
    };
    let (stored, sha): (Vec<u8>, String) = sqlx::query_as(
        "SELECT descriptor,sha256 FROM local_e2ee_image_preparation WHERE operation_id=?",
    )
    .bind(&change.change_id)
    .fetch_one(&mut *conn)
    .await?;
    valid(stored == *descriptor && change.payload["sha256"].as_str() == Some(&sha))?;
    let old = Descriptor::decode(&stored)?;
    let rows: Vec<(i64, Vec<u8>)> = sqlx::query_as("SELECT chunk_index,bytes FROM local_e2ee_image_staging WHERE operation_id=? ORDER BY chunk_index")
        .bind(&change.change_id).fetch_all(&mut *conn).await?;
    for (index, bytes) in &rows {
        old.verify_chunk(usize::try_from(*index)?, bytes)?;
    }
    let source = if rows.len() == old.artifact.chunks.len() {
        old.open(a, &rows.into_iter().map(|(_, b)| b).collect::<Vec<_>>())?
            .to_vec()
    } else {
        let source = read_source(blob_dir, &sha, old.artifact.total).await?;
        old.reconstruct(a, &source, &sha)?;
        source
    };
    valid(hex::encode(hash(&source)) == sha)?;
    let (next, records) = Descriptor::seal(a, &source)?;
    // The owning outbox stays present and the source pin is replaced in this transaction.
    sqlx::query("DELETE FROM local_e2ee_image_preparation WHERE operation_id=?")
        .bind(&change.change_id)
        .execute(&mut *conn)
        .await?;
    *descriptor = next.encode()?;
    sqlx::query("INSERT INTO local_e2ee_image_preparation(singleton,operation_id,descriptor,sha256) VALUES(1,?,?,?)")
        .bind(&change.change_id).bind(&*descriptor).bind(sha).execute(&mut *conn).await?;
    for (index, record) in records.iter().enumerate() {
        sqlx::query(
            "INSERT INTO local_e2ee_image_staging(operation_id,chunk_index,bytes) VALUES(?,?,?)",
        )
        .bind(&change.change_id)
        .bind(index as i64)
        .bind(record)
        .execute(&mut *conn)
        .await?;
    }
    Ok(projection)
}
/// The local plaintext image source is missing or no longer matches its hash.
#[derive(Debug)]
pub struct ImageSourceUnavailable;
impl std::fmt::Display for ImageSourceUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("error encrypted-image-source-unavailable")
    }
}
async fn read_source(blob_dir: &Path, sha: &str, total: u64) -> Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    async {
        let file =
            tokio::fs::File::open(crate::attachments::storage::object_path(blob_dir, sha)?).await?;
        let mut bytes = Vec::new();
        file.take(super::codec::IMAGE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .await?;
        valid(
            bytes.len() as u64 == total
                && bytes.len() <= super::codec::IMAGE_BYTES
                && hex::encode(hash(&bytes)) == sha,
        )?;
        anyhow::Ok(bytes)
    }
    .await
    .context(ImageSourceUnavailable)
}

pub struct Download {
    pub workspace: String,
    pub object: [u8; 32],
    pub descriptor_commitment: [u8; 32],
    pub chunk_count: usize,
    descriptor: Vec<u8>,
    sha256: String,
}
impl Database {
    /// Selects and advances past one pending object before transfer. The local
    /// cursor orders attempts, not acceptance, validation or content progress.
    pub async fn prepare_encrypted_image_download(
        &self,
        a: &Authority,
    ) -> Result<Option<Download>> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        super::super::client::validate_binding_and_cursor(&mut tx, a).await?;
        ensure!(
            super::super::client::initial_image_watermark(&mut tx).await? == "ready",
            "error encrypted-image-initial-catch-up"
        );
        let after = hex::decode(
            db::get_meta(&mut tx, DOWNLOAD_CURSOR)
                .await?
                .unwrap_or_default(),
        )
        .context("error encrypted-image-download-cursor")?;
        valid(after.is_empty() || after.len() == 32)?;
        let row: Option<(String, Vec<u8>, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT r.workspace,o.descriptor,o.sha256 {DOWNLOAD_CANDIDATES}
             GROUP BY o.object ORDER BY (o.object<=?),o.object LIMIT 1"
        )))
        .bind(after)
        .fetch_optional(&mut *tx)
        .await?;
        let result = row
            .map(|(workspace, bytes, sha256)| -> Result<_> {
                let d = Descriptor::decode(&bytes)?;
                Ok(Download {
                    workspace,
                    object: d.object,
                    descriptor_commitment: hash(&bytes),
                    chunk_count: d.artifact.chunks.len(),
                    descriptor: bytes,
                    sha256,
                })
            })
            .transpose()?;
        if let Some(download) = &result {
            db::set_meta(&mut tx, DOWNLOAD_CURSOR, &hex::encode(download.object)).await?;
        }
        tx.commit().await?;
        Ok(result)
    }
    /// Observes pending downloads without consuming a selection turn.
    pub async fn encrypted_image_download_pending(&self, a: &Authority) -> Result<bool> {
        let mut conn = self.acquire_reader().await?;
        super::super::client::validate_binding_and_cursor(&mut conn, a).await?;
        Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT EXISTS(SELECT 1 {DOWNLOAD_CANDIDATES})"
        )))
        .fetch_one(&mut *conn)
        .await?)
    }
    pub async fn install_encrypted_image(
        &self,
        a: &Authority,
        blob_dir: &Path,
        download: &Download,
        records: &[Vec<u8>],
        policy: crate::attachments::LifecyclePolicy,
    ) -> Result<()> {
        let d = Descriptor::decode(&download.descriptor)?;
        let plain = d.open(a, records)?;
        valid(hex::encode(hash(&plain)) == download.sha256)?;
        self.install_image_plaintext(a, blob_dir, download, plain.to_vec(), policy)
            .await
    }
    pub async fn complete_encrypted_image_from_local(
        &self,
        a: &Authority,
        blob_dir: &Path,
        download: &Download,
        policy: crate::attachments::LifecyclePolicy,
    ) -> Result<bool> {
        let d = Descriptor::decode(&download.descriptor)?;
        d.authority(a)?;
        match read_source(blob_dir, &download.sha256, d.artifact.total).await {
            Ok(bytes) => {
                self.install_image_plaintext(a, blob_dir, download, bytes, policy)
                    .await?;
                Ok(true)
            }
            Err(_) => Ok(false),
        }
    }
    async fn install_image_plaintext(
        &self,
        a: &Authority,
        blob_dir: &Path,
        download: &Download,
        plain: Vec<u8>,
        policy: crate::attachments::LifecyclePolicy,
    ) -> Result<()> {
        let d = Descriptor::decode(&download.descriptor)?;
        let image = crate::attachments::decode::validate_image(plain, None)
            .await
            .map_err(|_| anyhow::anyhow!("error encrypted-image-invalid"))?;
        let mut conn = self.acquire_writer().await?;
        super::super::client::validate_binding_and_cursor(&mut conn, a).await?;
        let reservation = crate::attachments::lifecycle::ensure_local_capacity(
            &mut conn,
            blob_dir,
            &download.sha256,
            image.bytes.len() as i64,
            policy,
            &crate::attachments::lifecycle::SystemClock,
        )
        .await?;
        let result = async {
            let mut tx = begin_immediate(&mut conn).await?;
            super::super::client::validate_binding_and_cursor(&mut tx, a).await?;
            let descriptor: Vec<u8> = sqlx::query_scalar(
                "SELECT descriptor FROM local_e2ee_image_objects WHERE object=? AND sha256=?",
            )
            .bind(d.object.as_slice())
            .bind(&download.sha256)
            .fetch_one(&mut *tx)
            .await?;
            valid(descriptor == download.descriptor)?;
            let facts: Vec<(i64, String, Option<i64>, Option<i64>)> = sqlx::query_as(
                "SELECT byte_size,media_type,width,height FROM task_attachments WHERE sha256=?",
            )
            .bind(&download.sha256)
            .fetch_all(&mut *tx)
            .await?;
            for (size, media, width, height) in facts {
                valid(
                    size == image.bytes.len() as i64
                        && media == image.facts.media_type
                        && width == Some(image.facts.width)
                        && height == Some(image.facts.height),
                )?;
            }
            let stored =
                crate::attachments::storage::stage_blob(blob_dir, &download.sha256, &image.bytes)
                    .await?;
            crate::attachments::storage::upsert_inventory_available(
                &mut tx,
                &download.sha256,
                stored.byte_size,
                &image.facts.media_type,
            )
            .await?;
            crate::attachments::lifecycle::reconcile_liveness_for_hashes_in_transaction(
                &mut tx,
                std::slice::from_ref(&download.sha256),
                &crate::attachments::lifecycle::SystemClock,
            )
            .await?;
            sqlx::query("UPDATE local_e2ee_image_objects SET verified=1 WHERE object=?")
                .bind(d.object.as_slice())
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            Ok(())
        }
        .await;
        if let Some(reservation) = reservation {
            crate::attachments::lifecycle::release_reservation(&mut conn, &reservation).await?;
        }
        result
    }
    pub async fn encrypted_image_upload_pending(&self, a: &Authority) -> Result<bool> {
        let mut conn = self.acquire_reader().await?;
        super::super::client::validate_binding_and_cursor(&mut conn, a).await?;
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM changes WHERE server_seq IS NULL AND op_type='attachment_add')")
            .fetch_one(&mut *conn).await?)
    }
    pub async fn encrypted_images_unavailable(&self, a: &Authority) -> Result<bool> {
        let mut conn = self.acquire_reader().await?;
        super::super::client::validate_binding_and_cursor(&mut conn, a).await?;
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_e2ee_image_references r JOIN task_attachments a ON a.workspace_id=r.workspace AND a.attachment_id=r.reference JOIN tasks t ON t.workspace_id=a.workspace_id AND t.id=a.task_id WHERE r.object IS NULL AND a.deleted=0 AND t.deleted=0)").fetch_one(&mut *conn).await?)
    }
}

impl Database {
    pub async fn encrypted_tail_frozen_record(
        &self,
        a: &Authority,
    ) -> Result<Option<(String, Vec<u8>)>> {
        let mut conn = self.acquire_reader().await?;
        super::super::client::validate_binding_and_cursor(&mut conn, a).await?;
        let record: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT record FROM local_e2ee_outbox WHERE singleton=1")
                .fetch_optional(&mut *conn)
                .await?;
        record
            .map(|r| Ok((codec::open(a, &r)?.change_id, r)))
            .transpose()
    }
}

impl Database {
    /// Exact targeted repair uses only an authenticated admitted mapping.
    pub async fn repair_encrypted_image(
        &self,
        a: &Authority,
        blob_dir: &Path,
        workspace: &str,
        reference: &str,
    ) -> Result<Upload> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        super::super::client::validate_binding_and_cursor(&mut tx, a).await?;
        ensure!(!a.rotation_pending(), "error membership-rotation-pending");
        let (descriptor,sha):(Vec<u8>,String)=sqlx::query_as("SELECT o.descriptor,o.sha256 FROM local_e2ee_image_references r JOIN local_e2ee_image_objects o ON o.object=r.object WHERE r.workspace=? AND r.reference=?").bind(workspace).bind(reference).fetch_optional(&mut *tx).await?.context("error encrypted-image-unavailable")?;
        let d = Descriptor::decode(&descriptor)?;
        let source = read_source(blob_dir, &sha, d.artifact.total).await?;
        let records = d.reconstruct(a, &source, &sha)?;
        tx.commit().await?;
        Ok(Upload {
            workspace: workspace.into(),
            object: d.object,
            commitment: hash(&descriptor),
            descriptor,
            records,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn valid_image_aead_does_not_make_invalid_plaintext_available() {
        let root = tempfile::tempdir().unwrap();
        let db = Database::open(&root.path().join("test.sqlite"))
            .await
            .unwrap();
        let a = crate::sync::encrypted_tail::tests::authority();
        let plain = b"not an image";
        let (d, records) = Descriptor::seal(&a, plain).unwrap();
        assert_eq!(d.open(&a, &records).unwrap().as_slice(), plain);
        let bytes = d.encode().unwrap();
        let download = Download {
            workspace: "0000000000000000".into(),
            object: d.object,
            descriptor_commitment: hash(&bytes),
            chunk_count: records.len(),
            descriptor: bytes,
            sha256: hex::encode(hash(plain)),
        };
        let error = db
            .install_encrypted_image(&a, root.path(), &download, &records, Default::default())
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "error encrypted-image-invalid");
        assert!(!root.path().join("objects").exists());
    }
}
