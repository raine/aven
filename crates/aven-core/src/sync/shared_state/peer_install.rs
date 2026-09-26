//! Published reads and fresh-peer installation. Enrollment authority is supplied
//! by the protected host, never recovered from a SQLite receipt.
use super::*;
use crate::db::installation::InstallationGuard;
use crate::sync::{
    bootstrap_staging::Component,
    seed_claim::{membership::VerifiedEnrollment, peer},
};
use sha2::{Digest, Sha256};

impl Database {
    /// Copies one published record inside the current-membership transaction.
    /// Image bytes are read from lifecycle ownership, never staging or restored.
    pub async fn published_snapshot_read(
        &self,
        auth: &peer::Authentication<'_>,
        descriptor: [u8; 32],
        component: Option<Component>,
        index: u64,
    ) -> Result<Vec<u8>> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        let current =
            crate::sync::seed_claim::membership::persistence::current(self, &mut tx).await?;
        current.membership.authenticate(auth, false)?;
        let binding = current.membership.publication().binding();
        ensure!(
            binding.descriptor_commitment == descriptor,
            "error snapshot-publication-mismatch"
        );
        let bytes = match component {
            None => {
                ensure!(index == 0, "error snapshot-index");
                current.evidence.descriptor.clone()
            }
            Some(Component::Image(object)) => {
                // SQLite owns the bytes. A prune transaction cannot invalidate
                // this owned response buffer, and this read creates no pin.
                sqlx::query_scalar("SELECT c.bytes FROM server_e2ee_image_chunks c JOIN server_e2ee_images i ON i.object=c.object WHERE i.bootstrap=? AND c.object=? AND c.chunk_index=?")
                    .bind(binding.bootstrap_id.as_slice()).bind(object.as_slice())
                    .bind(i64::try_from(index)?).fetch_optional(&mut *tx).await?
                    .context("error snapshot-image-unavailable")?
            }
            Some(component) => {
                sqlx::query_scalar("SELECT bytes FROM server_bootstrap_chunks WHERE bootstrap=? AND component=? AND chunk_index=?")
                    .bind(binding.bootstrap_id.as_slice()).bind(component.key())
                    .bind(i64::try_from(index)?).fetch_optional(&mut *tx).await?
                    .context("error snapshot-artifact-unavailable")?
            }
        };
        ensure!(
            bytes.len() <= crate::sync::bootstrap_staging::MAX_REQUEST_BYTES,
            "error snapshot-limit"
        );
        tx.commit().await?;
        Ok(bytes)
    }

    /// Receipt lookup still requires independently verified protected authority.
    /// It never repairs a receipt or resets a later cursor or domain edit.
    pub async fn peer_snapshot_receipt(
        &self,
        verified: &VerifiedEnrollment,
        enrollment: [u8; 32],
        client: &str,
        guard: &InstallationGuard,
    ) -> Result<Option<SharedStateInstallReport>> {
        ensure!(
            self.file_identity() == Some(guard.identity()),
            "error snapshot-installation"
        );
        let mut conn = self.acquire_writer().await?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        validate_target_identity(&mut tx, enrollment, client).await?;
        let receipt = receipt(&mut tx, verified, enrollment, client).await?;
        tx.commit().await?;
        Ok(receipt)
    }

    /// Authenticated metadata and mappings commit independently of image bytes.
    pub async fn install_peer_snapshot(
        &self,
        verified: &VerifiedEnrollment,
        enrollment: [u8; 32],
        client: &str,
        guard: &InstallationGuard,
        package: &bootstrap_format::download::Metadata,
    ) -> Result<SharedStateInstallReport> {
        if let Some(report) = self
            .peer_snapshot_receipt(verified, enrollment, client, guard)
            .await?
        {
            return Ok(report);
        }
        let binding = verified.publication().binding();
        ensure!(
            Sha256::digest(&package.descriptor).as_slice() == binding.descriptor_commitment,
            "error snapshot-publication-mismatch"
        );
        let bootstrap_format::download::VerifiedMetadata { mut capture, index } =
            bootstrap_format::download::decrypt(package, verified.key())?;
        for blob in &mut capture.snapshot.tables.blob_inventory {
            blob.available = 0;
            blob.last_verified_at = None;
        }
        let mut conn = self.acquire_writer().await?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        validate_target_identity(&mut tx, enrollment, client).await?;
        if let Some(report) = receipt(&mut tx, verified, enrollment, client).await? {
            tx.commit().await?;
            return Ok(report);
        }
        ensure_empty_domain(&mut tx).await?;
        let occupied: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_seed_source) OR EXISTS(SELECT 1 FROM local_seed_publication_intent) OR EXISTS(SELECT 1 FROM local_seed_genesis_pin) OR EXISTS(SELECT 1 FROM server_seed_claim) OR EXISTS(SELECT 1 FROM meta WHERE key IN ('sync_server_url','e2ee_association'))").fetch_one(&mut *tx).await?;
        ensure!(!occupied, "error snapshot-target-not-fresh");
        let report = install_in_transaction(&mut tx, &capture).await?;
        crate::sync::crash::Crash::Snapshot.at("metadata");
        let hashes = capture
            .snapshot
            .tables
            .blob_inventory
            .iter()
            .map(|b| b.sha256.clone())
            .collect::<Vec<_>>();
        crate::attachments::lifecycle::reconcile_liveness_for_hashes_in_transaction(
            &mut tx,
            &hashes,
            &crate::attachments::lifecycle::SystemClock,
        )
        .await?;
        let generation = db::get_meta(&mut tx, "sync_generation")
            .await?
            .unwrap_or_else(|| "0".into())
            .parse::<i64>()?
            .checked_add(1)
            .context("error snapshot-generation")?;
        let association = association(verified);
        crate::sync::encrypted_tail::dependencies::initialize(
            &mut tx,
            &association,
            generation,
            i64::try_from(binding.prefix_count)?,
            &capture.snapshot.tables.task_dependencies,
        )
        .await?;
        crate::sync::encrypted_tail::attachments::client::initialize_index(
            &mut tx,
            &association,
            generation,
            i64::try_from(binding.prefix_count)?,
            &binding.descriptor_commitment,
            index,
            false,
        )
        .await?;
        db::set_meta(&mut tx, "sync_generation", &generation.to_string()).await?;
        db::set_meta(&mut tx, "sync_cursor", &binding.prefix_count.to_string()).await?;
        db::set_meta(&mut tx, "e2ee_association", &association).await?;
        sqlx::query("INSERT INTO local_peer_snapshot_install(singleton,enrollment,checkpoint,descriptor,stream,prefix_count,client_id,association,sync_generation,attachment_count) VALUES(1,?,?,?,?,?,?,?,?,?)")
            .bind(enrollment.as_slice()).bind(verified.checkpoint().as_slice()).bind(binding.descriptor_commitment.as_slice()).bind(binding.stream_id.as_slice()).bind(i64::try_from(binding.prefix_count)?).bind(client).bind(association).bind(generation).bind(i64::try_from(report.attachment_count)?).execute(&mut *tx).await?;
        crate::sync::crash::Crash::Snapshot.at("before-commit");
        tx.commit().await?;
        crate::sync::crash::Crash::Snapshot.at("after-commit");
        Ok(report)
    }
}

fn association(v: &VerifiedEnrollment) -> String {
    let b = v.publication().binding();
    format!(
        "{}:{}:{}",
        hex::encode(b.vault_id),
        hex::encode(b.stream_id),
        hex::encode(b.bootstrap_id)
    )
}
async fn validate_target_identity(
    conn: &mut sqlx::SqliteConnection,
    enrollment: [u8; 32],
    client: &str,
) -> Result<()> {
    let matches: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_peer_enrollment WHERE identity=? AND client_id=? AND role='peer' AND client_id=(SELECT value FROM meta WHERE key='client_id'))").bind(enrollment.as_slice()).bind(client).fetch_one(conn).await?;
    ensure!(matches, "error snapshot-enrollment-mismatch");
    Ok(())
}
async fn receipt(
    conn: &mut sqlx::SqliteConnection,
    verified: &VerifiedEnrollment,
    enrollment: [u8; 32],
    client: &str,
) -> Result<Option<SharedStateInstallReport>> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_peer_snapshot_install)")
            .fetch_one(&mut *conn)
            .await?;
    if !exists {
        return Ok(None);
    }
    let b = verified.publication().binding();
    let count: Option<i64> = sqlx::query_scalar("SELECT attachment_count FROM local_peer_snapshot_install WHERE singleton=1 AND enrollment=? AND checkpoint=? AND descriptor=? AND stream=? AND prefix_count=? AND client_id=? AND association=? AND association=(SELECT value FROM meta WHERE key='e2ee_association') AND sync_generation=CAST((SELECT value FROM meta WHERE key='sync_generation') AS INTEGER) AND CAST((SELECT value FROM meta WHERE key='sync_cursor') AS INTEGER)>=prefix_count")
        .bind(enrollment.as_slice()).bind(verified.checkpoint().as_slice()).bind(b.descriptor_commitment.as_slice()).bind(b.stream_id.as_slice()).bind(i64::try_from(b.prefix_count)?).bind(client).bind(association(verified)).fetch_optional(&mut *conn).await?;
    crate::sync::encrypted_tail::dependencies::validate(
        conn,
        &association(verified),
        i64::try_from(b.prefix_count)?,
    )
    .await?;
    crate::sync::encrypted_tail::attachments::client::validate(
        conn,
        &association(verified),
        i64::try_from(b.prefix_count)?,
        &b.descriptor_commitment,
    )
    .await?;
    Ok(Some(SharedStateInstallReport {
        prefix_count: b.prefix_count,
        attachment_count: u64::try_from(count.context("error snapshot-receipt-mismatch")?)?,
    }))
}
