use super::*;
use crate::db::{self, Database, begin_immediate, installation::InstallationGuard};
use anyhow::Context;
use sqlx::SqliteConnection;
use std::collections::HashMap;

/// Explicit current-head request context. Historical outcomes cannot authenticate it.
pub struct Authentication<'a> {
    pub vault: [u8; 32],
    pub genesis: [u8; 32],
    pub device: [u8; 32],
    pub head: [u8; 32],
    pub bearer: &'a Secret,
}

impl Database {
    pub async fn peer_target_preflight(&self) -> Result<String> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        fresh(&mut tx).await?;
        let client = db::get_meta(&mut tx, "client_id")
            .await?
            .context("error enrollment-client-missing")?;
        tx.commit().await?;
        Ok(client)
    }
    /// Rechecks that this pinned peer join is unfinished: nothing installed,
    /// associated or set up, and the domain still empty. The enrollment pin
    /// and installation fence stay in place.
    pub async fn peer_retry_preflight(
        &self,
        identity: [u8; 32],
        client: &str,
        guard: &InstallationGuard,
    ) -> Result<()> {
        check(self.file_identity() == Some(guard.identity()))?;
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let pinned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_peer_enrollment WHERE identity=? AND client_id=? AND role='peer' AND client_id=(SELECT value FROM meta WHERE key='client_id'))").bind(identity.as_slice()).bind(client).fetch_one(&mut *tx).await?;
        ensure!(pinned, "error enrollment-pin");
        let occupied: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_peer_snapshot_install) OR EXISTS(SELECT 1 FROM local_seed_source) OR EXISTS(SELECT 1 FROM local_seed_publication_intent) OR EXISTS(SELECT 1 FROM local_seed_genesis_pin) OR EXISTS(SELECT 1 FROM server_seed_claim) OR EXISTS(SELECT 1 FROM meta WHERE key IN ('sync_server_url','e2ee_association'))").fetch_one(&mut *tx).await?;
        ensure!(!occupied, "error enrollment-retry-unavailable");
        crate::sync::shared_state::ensure_empty_domain(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn enrollment_pin(&self) -> Result<Option<([u8; 32], String, String)>> {
        let mut conn = self.acquire_reader().await?;
        let row: Option<(Vec<u8>, String, String)> = sqlx::query_as(
            "SELECT identity,client_id,role FROM local_peer_enrollment WHERE singleton=1",
        )
        .fetch_optional(&mut *conn)
        .await?;
        row.map(|(id, client, role)| {
            Ok((
                id.try_into()
                    .map_err(|_| anyhow::anyhow!("error enrollment-pin"))?,
                client,
                role,
            ))
        })
        .transpose()
    }
    pub async fn pin_enrollment(
        &self,
        identity: [u8; 32],
        client: &str,
        role: &str,
        guard: &InstallationGuard,
    ) -> Result<()> {
        check(self.file_identity() == Some(guard.identity()))?;
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let stored: Option<(Vec<u8>, String, String)> = sqlx::query_as(
            "SELECT identity,client_id,role FROM local_peer_enrollment WHERE singleton=1",
        )
        .fetch_optional(&mut *tx)
        .await?;
        check(db::get_meta(&mut tx, "client_id").await?.as_deref() == Some(client))?;
        if let Some((id, c, r)) = stored {
            check(id == identity && c == client && r == role)?;
        } else {
            if role == "peer" {
                fresh(&mut tx).await?;
            } else {
                ensure_adopted(&mut tx).await?;
            }
            sqlx::query("INSERT INTO local_peer_enrollment(singleton,identity,client_id,role) VALUES(1,?,?,?)").bind(identity.as_slice()).bind(client).bind(role).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn adopted_enrollment_client(&self) -> Result<String> {
        let mut conn = self.acquire_reader().await?;
        ensure_adopted(&mut conn).await?;
        db::get_meta(&mut conn, "client_id")
            .await?
            .context("error enrollment-client-missing")
    }
    pub async fn enrollment_artifact(&self, kind: &str) -> Result<Option<Vec<u8>>> {
        let mut conn = self.acquire_reader().await?;
        Ok(sqlx::query_scalar(
            "SELECT commitment FROM local_peer_enrollment_artifacts WHERE kind=?",
        )
        .bind(kind)
        .fetch_optional(&mut *conn)
        .await?)
    }
    pub async fn enrollment_artifacts(&self) -> Result<HashMap<String, Vec<u8>>> {
        let mut conn = self.acquire_reader().await?;
        let rows: Vec<(String, Vec<u8>)> =
            sqlx::query_as("SELECT kind, commitment FROM local_peer_enrollment_artifacts")
                .fetch_all(&mut *conn)
                .await?;
        Ok(rows.into_iter().collect())
    }
    /// Monotonic, nonsecret marker for write-once enrollment state changes.
    pub async fn enrollment_artifact_marker(&self) -> Result<i64> {
        let mut conn = self.acquire_reader().await?;
        Ok(
            sqlx::query_scalar("SELECT count(*) FROM local_peer_enrollment_artifacts")
                .fetch_one(&mut *conn)
                .await?,
        )
    }
    pub async fn pin_enrollment_artifact(
        &self,
        identity: [u8; 32],
        kind: &str,
        digest: [u8; 32],
    ) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_peer_enrollment WHERE identity=? AND client_id=(SELECT value FROM meta WHERE key='client_id'))").bind(identity.as_slice()).fetch_one(&mut *tx).await?;
        check(valid)?;
        sqlx::query(
            "INSERT OR IGNORE INTO local_peer_enrollment_artifacts(kind,commitment) VALUES(?,?)",
        )
        .bind(kind)
        .bind(digest.as_slice())
        .execute(&mut *tx)
        .await?;
        let saved: Vec<u8> = sqlx::query_scalar(
            "SELECT commitment FROM local_peer_enrollment_artifacts WHERE kind=?",
        )
        .bind(kind)
        .fetch_one(&mut *tx)
        .await?;
        check(saved == digest)?;
        tx.commit().await?;
        Ok(())
    }
}
async fn ensure_adopted(conn: &mut SqliteConnection) -> Result<()> {
    let ready: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_seed_publication_intent WHERE state='adopted' AND association=(SELECT value FROM meta WHERE key='e2ee_association') AND association_generation=CAST((SELECT value FROM meta WHERE key='sync_generation') AS INTEGER))").fetch_one(conn).await?;
    ensure!(ready, "error enrollment-inviter-not-adopted");
    Ok(())
}
async fn fresh(conn: &mut SqliteConnection) -> Result<()> {
    crate::sync::shared_state::ensure_empty_target(conn).await?;
    let occupied: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_seed_genesis_pin) OR EXISTS(SELECT 1 FROM server_seed_claim) OR EXISTS(SELECT 1 FROM local_peer_enrollment) OR EXISTS(SELECT 1 FROM meta WHERE key IN ('sync_server_url','e2ee_association'))").fetch_one(conn).await?;
    ensure!(!occupied, "error enrollment-target-not-fresh");
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegistrationStatus {
    Open,
    Expired,
    Consumed,
}
