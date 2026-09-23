use super::*;
use crate::db::{self, Database, begin_immediate, installation::InstallationGuard};
use anyhow::Context;
use sqlx::SqliteConnection;
type InvitationRow = (Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>);

/// Explicit current-head request context. Historical outcomes cannot authenticate it.
pub struct Authentication<'a> {
    pub vault: [u8; 32],
    pub genesis: [u8; 32],
    pub device: [u8; 32],
    pub credential_version: u32,
    pub head: [u8; 32],
    pub bearer: &'a Secret,
}

pub(crate) struct Current {
    pub genesis: Genesis,
    pub publication: Publication,
    pub descriptor: Vec<u8>,
    pub admission: Option<Admission>,
}
impl Current {
    fn head(&self) -> [u8; 32] {
        self.admission
            .as_ref()
            .map_or(self.publication.commitment(), Admission::commitment)
    }
    fn authenticate(&self, auth: &Authentication<'_>, require_head: bool) -> Result<()> {
        check(
            auth.credential_version == 1
                && auth.vault == self.genesis.context.vault_id
                && auth.genesis == self.genesis.commitment(),
        )?;
        ensure!(
            auth.head == self.head()
                || (!require_head && auth.head == self.publication.commitment()),
            "error enrollment-context-stale"
        );
        let verifier = if auth.device == self.genesis.device {
            self.genesis.verifier
        } else {
            self.admission
                .as_ref()
                .filter(|a| a.recipient.device == auth.device)
                .map(|a| a.recipient.verifier)
                .context("error enrollment-unauthorized")?
        };
        ensure!(
            bool::from(credential_verifier(auth.vault, auth.device, auth.bearer).ct_eq(&verifier)),
            "error enrollment-unauthorized"
        );
        Ok(())
    }
}

pub(crate) async fn current(conn: &mut SqliteConnection) -> Result<Current> {
    let (raw, genesis_only): (Vec<u8>, bool) =
        sqlx::query_as("SELECT genesis, genesis_only FROM server_seed_claim WHERE singleton=1")
            .fetch_optional(&mut *conn)
            .await?
            .context("error enrollment-unauthorized")?;
    ensure!(!genesis_only, "error enrollment-requires-ready");
    let genesis = Genesis::from_record(&raw)?;
    let (descriptor, raw): (Vec<u8>, Vec<u8>) = sqlx::query_as(
        "SELECT descriptor, signed_record FROM server_bootstrap_publication WHERE singleton=1",
    )
    .fetch_optional(&mut *conn)
    .await?
    .context("error enrollment-requires-ready")?;
    let publication = Publication::from_record(&genesis, &descriptor, &raw)?;
    let (seq, head): (i64, Vec<u8>) = sqlx::query_as(
        "SELECT sequence, commitment FROM server_e2ee_membership_head WHERE singleton=1",
    )
    .fetch_optional(&mut *conn)
    .await?
    .context("error enrollment-membership-unsupported")?;
    let saved: Option<InvitationRow> = sqlx::query_as(
        "SELECT declaration, request, admission FROM server_peer_invitation WHERE singleton=1",
    )
    .fetch_optional(&mut *conn)
    .await?;
    let admission = match seq {
        1 => {
            ensure!(
                head == publication.commitment() && saved.as_ref().is_none_or(|r| r.2.is_none()),
                "error enrollment-membership-unsupported"
            );
            None
        }
        2 => {
            let (declaration, request, admission) =
                saved.context("error enrollment-membership-unsupported")?;
            let declaration = Declaration::from_record(&genesis, &publication, &declaration)?;
            let admission = Admission::from_record(
                &genesis,
                &publication,
                &declaration,
                &request.context("error enrollment-storage")?,
                &admission.context("error enrollment-storage")?,
            )?;
            ensure!(
                head == admission.commitment(),
                "error enrollment-membership-unsupported"
            );
            Some(admission)
        }
        _ => anyhow::bail!("error enrollment-membership-unsupported"),
    };
    Ok(Current {
        genesis,
        publication,
        descriptor,
        admission,
    })
}

fn now() -> Result<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .context("error enrollment-clock")
}
async fn advance_clock(
    conn: &mut SqliteConnection,
    d: &Declaration,
    observed: i64,
) -> Result<bool> {
    ensure!(observed >= 0, "error enrollment-clock");
    sqlx::query("UPDATE server_peer_invitation SET clock_high_water=max(clock_high_water, ?), expired=CASE WHEN expired=1 OR max(clock_high_water, ?) >= ? THEN 1 ELSE 0 END WHERE singleton=1 AND admission IS NULL")
        .bind(observed).bind(observed).bind(i64::try_from(d.expiry)?).execute(&mut *conn).await?;
    Ok(
        sqlx::query_scalar("SELECT expired FROM server_peer_invitation WHERE singleton=1")
            .fetch_one(conn)
            .await?,
    )
}

impl Database {
    pub async fn register_peer_invitation(
        &self,
        auth: &Authentication<'_>,
        raw: &[u8],
    ) -> Result<RegistrationStatus> {
        self.register_peer_invitation_at(auth, raw, now()?).await
    }
    pub(super) async fn register_peer_invitation_at(
        &self,
        auth: &Authentication<'_>,
        raw: &[u8],
        now: i64,
    ) -> Result<RegistrationStatus> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let c = current(&mut tx).await?;
        c.authenticate(auth, false)?;
        ensure!(
            auth.device == c.genesis.device,
            "error enrollment-unauthorized"
        );
        let d = Declaration::from_record(&c.genesis, &c.publication, raw)?;
        let stored: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT declaration FROM server_peer_invitation WHERE singleton=1")
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(stored) = stored {
            ensure!(
                stored == raw,
                "error enrollment-first-peer-subset-slot-retained"
            );
            advance_clock(&mut tx, &d, now).await?;
        } else {
            c.authenticate(auth, true)?;
            ensure!(c.admission.is_none(), "error enrollment-first-peer-subset");
            let expiry = i64::try_from(d.expiry)?;
            ensure!(
                now >= 0 && (expiry <= now || expiry - now <= 3600),
                "error enrollment-expired"
            );
            sqlx::query("INSERT INTO server_peer_invitation(singleton,declaration,clock_high_water,expired) VALUES(1,?,?,?)").bind(raw).bind(now).bind(expiry<=now).execute(&mut *tx).await?;
        }
        let (expired, consumed): (bool, bool) = sqlx::query_as(
            "SELECT expired,admission IS NOT NULL FROM server_peer_invitation WHERE singleton=1",
        )
        .fetch_one(&mut *tx)
        .await?;
        let status = if consumed {
            RegistrationStatus::Consumed
        } else if expired {
            RegistrationStatus::Expired
        } else {
            RegistrationStatus::Open
        };
        tx.commit().await?;
        Ok(status)
    }
    /// Bounded mailbox posting only. Possessing a handle grants no data access.
    pub async fn post_peer_request(
        &self,
        vault: [u8; 32],
        handle: [u8; 32],
        request: &[u8],
    ) -> Result<()> {
        request_parts(request, &handle)?;
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let c = current(&mut tx).await?;
        check(vault == c.genesis.context.vault_id)?;
        let (raw, stored): (Vec<u8>, Option<Vec<u8>>) = sqlx::query_as(
            "SELECT declaration,request FROM server_peer_invitation WHERE singleton=1",
        )
        .fetch_optional(&mut *tx)
        .await?
        .context("error enrollment-unavailable")?;
        let d = Declaration::from_record(&c.genesis, &c.publication, &raw)?;
        check(handle == d.handle)?;
        if let Some(stored) = stored {
            ensure!(stored == request, "error enrollment-request-occupied");
        } else {
            let expired = advance_clock(&mut tx, &d, now()?).await?;
            if expired {
                tx.commit().await?;
                anyhow::bail!("error enrollment-expired");
            }
            sqlx::query("UPDATE server_peer_invitation SET request=? WHERE singleton=1")
                .bind(request)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn peer_mailbox(&self, vault: [u8; 32], handle: [u8; 32]) -> Result<Mailbox> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let c = current(&mut tx).await?;
        check(vault == c.genesis.context.vault_id)?;
        let (raw, request, admission): InvitationRow = sqlx::query_as(
            "SELECT declaration,request,admission FROM server_peer_invitation WHERE singleton=1",
        )
        .fetch_optional(&mut *tx)
        .await?
        .context("error enrollment-unavailable")?;
        let d = Declaration::from_record(&c.genesis, &c.publication, &raw)?;
        check(handle == d.handle)?;
        let evidence = admission.map(|admission| Evidence {
            genesis: c.genesis.record().to_vec(),
            publication: c.publication.record().to_vec(),
            declaration: raw,
            request: request.clone().unwrap_or_default(),
            admission,
        });
        tx.commit().await?;
        Ok(Mailbox { request, evidence })
    }
    pub async fn admit_first_peer(
        &self,
        auth: &Authentication<'_>,
        record: &[u8],
    ) -> Result<Evidence> {
        self.admit_first_peer_at(auth, record, now()?).await
    }
    pub(super) async fn admit_first_peer_at(
        &self,
        auth: &Authentication<'_>,
        record: &[u8],
        now: i64,
    ) -> Result<Evidence> {
        check(record.len() == ADMISSION_BYTES)?;
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let c = current(&mut tx).await?;
        c.authenticate(auth, false)?;
        let (raw, request, saved): InvitationRow = sqlx::query_as(
            "SELECT declaration,request,admission FROM server_peer_invitation WHERE singleton=1",
        )
        .fetch_optional(&mut *tx)
        .await?
        .context("error enrollment-unavailable")?;
        let d = Declaration::from_record(&c.genesis, &c.publication, &raw)?;
        let request = request.context("error enrollment-request-missing")?;
        let a = Admission::from_record(&c.genesis, &c.publication, &d, &request, record)?;
        if let Some(saved) = saved {
            ensure!(saved == record, "error enrollment-conflict");
        } else {
            c.authenticate(auth, true)?;
            ensure!(
                c.admission.is_none() && auth.device == c.genesis.device,
                "error enrollment-first-peer-subset"
            );
            if advance_clock(&mut tx, &d, now).await? {
                tx.commit().await?;
                anyhow::bail!("error enrollment-expired");
            }
            sqlx::query("UPDATE server_peer_invitation SET admission=? WHERE singleton=1")
                .bind(record)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE server_e2ee_membership_head SET sequence=2,commitment=? WHERE singleton=1 AND sequence=1").bind(a.commitment().as_slice()).execute(&mut *tx).await?;
        }
        let result = Evidence {
            genesis: c.genesis.record().to_vec(),
            publication: c.publication.record().to_vec(),
            declaration: raw,
            request,
            admission: record.to_vec(),
        };
        tx.commit().await?;
        Ok(result)
    }
    /// Only the published descriptor, needed to finish enrollment verification.
    /// This is not a bootstrap chunk/image retrieval API.
    pub async fn peer_enrollment_descriptor(&self, auth: &Authentication<'_>) -> Result<Vec<u8>> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let c = current(&mut tx).await?;
        c.authenticate(auth, true)?;
        ensure!(c.admission.is_some(), "error enrollment-not-admitted");
        tx.commit().await?;
        Ok(c.descriptor)
    }
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

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mailbox {
    pub request: Option<Vec<u8>>,
    pub evidence: Option<Evidence>,
}

#[cfg(test)]
pub(super) fn test_now() -> i64 {
    now().unwrap()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegistrationStatus {
    Open,
    Expired,
    Consumed,
}
