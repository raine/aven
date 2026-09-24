//! Bounded signed history, invitation outcomes and vault-clock transactions.
use super::super::peer::{Authentication, RegistrationStatus, request_parts};
use super::*;
use crate::db::{Database, begin_immediate};
use sqlx::SqliteConnection;

pub const MAX_INVITATIONS: usize = 128;
pub const MAX_CANDIDATES: usize = 8;
pub(crate) struct Current {
    pub membership: Membership,
    pub evidence: Evidence,
}
/// Bound stored byte lengths before materializing any attacker-influenced blobs.
pub(crate) async fn current(conn: &mut SqliteConnection) -> Result<Current> {
    let (invitations, malformed): (i64, i64) = sqlx::query_as("SELECT count(*),coalesce(sum(CASE WHEN length(declaration)!=280 OR (request IS NOT NULL AND length(request)!=314) OR length(handle)!=32 OR length(inviter)!=32 THEN 1 ELSE 0 END),0) FROM server_membership_invitations")
        .fetch_one(&mut *conn).await?;
    ensure!(
        invitations <= MAX_INVITATIONS as i64 && malformed == 0,
        "error membership-limit"
    );
    let clock: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM server_membership_clock WHERE high_water>=0)",
    )
    .fetch_one(&mut *conn)
    .await?;
    ensure!(invitations == 0 || clock, "error enrollment-clock-missing");
    let sizes: Option<(i64, i64, i64, bool)> = sqlx::query_as("SELECT length(s.genesis),length(p.signed_record),length(p.descriptor),s.genesis_only FROM server_seed_claim s JOIN server_bootstrap_publication p ON p.singleton=s.singleton WHERE s.singleton=1")
        .fetch_optional(&mut *conn).await?;
    let (g, p, d, only) = sizes.context("error enrollment-requires-ready")?;
    ensure!(
        !only
            && g == GENESIS_BYTES as i64
            && p == PUBLICATION_BYTES as i64
            && (0..=1024).contains(&d),
        "error membership-storage"
    );
    let (count, total, largest): (i64, i64, i64) = sqlx::query_as("SELECT count(*),coalesce(sum(length(record) + CASE WHEN handle IS NULL THEN 0 ELSE ? END),0),coalesce(max(length(record)),0) FROM server_membership_transitions")
        .bind((DECLARATION_BYTES + REQUEST_BYTES) as i64).fetch_one(&mut *conn).await?;
    ensure!(
        (0..=MAX_TRANSITIONS as i64).contains(&count)
            && largest <= MAX_RECORD_BYTES as i64
            && total >= 0
            && g + p + d + total <= MAX_CHAIN_BYTES as i64,
        "error membership-limit"
    );
    let (genesis, publication, descriptor): (Vec<u8>, Vec<u8>, Vec<u8>) = sqlx::query_as("SELECT s.genesis,p.signed_record,p.descriptor FROM server_seed_claim s JOIN server_bootstrap_publication p ON p.singleton=s.singleton WHERE s.singleton=1")
        .fetch_one(&mut *conn).await?;
    type Row = (i64, Option<Vec<u8>>, Vec<u8>);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT sequence,handle,record FROM server_membership_transitions ORDER BY sequence",
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut evidence = Evidence {
        genesis,
        publication,
        descriptor,
        transitions: Vec::with_capacity(rows.len()),
    };
    let mut membership = evidence.verify()?;
    for (seq, handle, record) in rows {
        ensure!(
            seq == membership.sequence() as i64 + 1,
            "error membership-history-missing"
        );
        let (declaration, request) = if let Some(handle) = handle {
            let (declaration, request, inviter, expiry, admitted): (Vec<u8>, Vec<u8>, Vec<u8>, i64, i64) =
                sqlx::query_as("SELECT declaration,request,inviter,expiry,admitted_sequence FROM server_membership_invitations WHERE handle=?")
                    .bind(&handle).fetch_one(&mut *conn).await?;
            let dec = Declaration::from_record(&membership, &declaration)?;
            ensure!(
                handle == dec.handle
                    && inviter == dec.inviter
                    && expiry >= 0
                    && expiry as u64 == dec.expiry()
                    && admitted == seq,
                "error membership-storage"
            );
            (declaration, request)
        } else {
            (vec![], vec![])
        };
        membership = membership.append(&declaration, &request, &record)?;
        evidence.transitions.push(EvidenceRecord {
            declaration,
            request,
            record,
        });
    }
    let (seq, head): (i64, Vec<u8>) = sqlx::query_as(
        "SELECT sequence,commitment FROM server_e2ee_membership_head WHERE singleton=1",
    )
    .fetch_one(&mut *conn)
    .await?;
    ensure!(
        seq == membership.sequence() as i64 && head == membership.head(),
        "error membership-head-invalid"
    );
    let orphans: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM server_membership_invitations i WHERE admitted_sequence IS NOT NULL AND NOT EXISTS(SELECT 1 FROM server_membership_transitions a WHERE a.handle=i.handle AND a.sequence=i.admitted_sequence))")
        .fetch_one(&mut *conn).await?;
    ensure!(!orphans, "error membership-history-missing");
    let invitations: Vec<(Vec<u8>, bool, bool, bool)> = sqlx::query_as("SELECT inviter,admitted_sequence IS NOT NULL,revoked,expired FROM server_membership_invitations")
        .fetch_all(&mut *conn).await?;
    for (inviter, admitted, revoked, expired) in invitations {
        let inviter: Hash = inviter
            .try_into()
            .map_err(|_| anyhow::anyhow!("error membership-storage"))?;
        ensure!(
            revoked == (!admitted && membership.member(&inviter).is_err()) && (!revoked || expired),
            "error membership-invitation-projection"
        );
    }
    Ok(Current {
        membership,
        evidence,
    })
}
fn now() -> Result<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .context("error enrollment-clock")
}
async fn observe(conn: &mut SqliteConnection, time: i64) -> Result<i64> {
    ensure!(time >= 0, "error enrollment-clock");
    sqlx::query("INSERT INTO server_membership_clock(singleton,high_water) VALUES(1,?) ON CONFLICT(singleton) DO UPDATE SET high_water=max(high_water,excluded.high_water)")
        .bind(time).execute(&mut *conn).await?;
    let high: i64 =
        sqlx::query_scalar("SELECT high_water FROM server_membership_clock WHERE singleton=1")
            .fetch_one(&mut *conn)
            .await?;
    sqlx::query("UPDATE server_membership_invitations SET expired=1 WHERE admitted_sequence IS NULL AND expiry<=?")
        .bind(high).execute(&mut *conn).await?;
    Ok(high)
}
/// Serialized outcome of cancelling a registered invitation. `Admitted` is a
/// hint only; callers resolve admission from the verified membership chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CancelStatus {
    Admitted,
    Cancelled,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagementPreparation {
    pub evidence: Evidence,
    pub high_water: u64,
}
async fn append_transition(
    conn: &mut SqliteConnection,
    before: &Membership,
    next: &Membership,
    handle: Option<Hash>,
    record: &[u8],
) -> Result<()> {
    sqlx::query("INSERT INTO server_membership_transitions(sequence,handle,record) VALUES(?,?,?)")
        .bind(next.sequence() as i64)
        .bind(handle.map(|h| h.to_vec()))
        .bind(record)
        .execute(&mut *conn)
        .await?;
    let changed = sqlx::query("UPDATE server_e2ee_membership_head SET sequence=?,commitment=? WHERE singleton=1 AND sequence=? AND commitment=?")
        .bind(next.sequence() as i64).bind(next.head().as_slice()).bind(before.sequence() as i64).bind(before.head().as_slice()).execute(conn).await?;
    ensure!(
        changed.rows_affected() == 1,
        "error membership-head-invalid"
    );
    Ok(())
}
pub(crate) async fn allocator(conn: &mut SqliteConnection, m: &Membership) -> Result<u64> {
    let b = m.publication().binding();
    let (n, high): (i64, i64) = sqlx::query_as(
        "SELECT prefix_count,high_water FROM server_e2ee_allocator WHERE singleton=1 AND stream=?",
    )
    .bind(b.stream_id.as_slice())
    .fetch_one(conn)
    .await?;
    ensure!(
        n >= 0
            && n as u64 == b.prefix_count
            && high >= n
            && high as u64 >= m.current_generation().starts_after,
        "error membership-allocator"
    );
    Ok(high as u64)
}
impl Database {
    pub async fn prepare_membership_management(
        &self,
        auth: &Authentication<'_>,
    ) -> Result<ManagementPreparation> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let c = current(&mut tx).await?;
        c.membership.authenticate(auth, false)?;
        let high_water = allocator(&mut tx, &c.membership).await?;
        tx.commit().await?;
        Ok(ManagementPreparation {
            evidence: c.evidence,
            high_water,
        })
    }
    pub async fn apply_membership_management(
        &self,
        auth: &Authentication<'_>,
        record: &[u8],
    ) -> Result<Vec<u8>> {
        ensure!(record.len() <= MAX_RECORD_BYTES, "error membership-limit");
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let c = current(&mut tx).await?;
        c.membership.authenticate(auth, false)?;
        let (core, _, _, _) = encoding::components(record)?;
        let (signer, action) = encoding::signer_action(core)?;
        ensure!(matches!(action, 4 | 5), "error membership-action");
        if !c.evidence.transitions.iter().any(|t| t.record == record) {
            ensure!(signer == auth.device, "error enrollment-unauthorized");
            let next = c.membership.append(&[], &[], record)?;
            let high = allocator(&mut tx, &c.membership).await?;
            ensure!(
                action != 5 || next.current_generation().starts_after == high,
                "error membership-cutoff"
            );
            append_transition(&mut tx, &c.membership, &next, None, record).await?;
            for member in &c.membership.members {
                if next.member(&member.device).is_err() {
                    sqlx::query("UPDATE server_membership_invitations SET revoked=1,expired=1 WHERE inviter=? AND admitted_sequence IS NULL")
                        .bind(member.device.as_slice()).execute(&mut *tx).await?;
                    sqlx::query("DELETE FROM server_e2ee_image_tickets WHERE device=?")
                        .bind(member.device.as_slice())
                        .execute(&mut *tx)
                        .await?;
                }
            }
            crate::sync::encrypted_tail::attachments::server::refresh(&mut tx, now()?).await?;
        }
        tx.commit().await?;
        Ok(record.to_vec())
    }
}
impl Database {
    /// This read alone accepts a verified ancestor head, after current credential authentication.
    pub async fn membership_evidence(&self, auth: &Authentication<'_>) -> Result<Evidence> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let c = current(&mut tx).await?;
        c.membership.authenticate(auth, true)?;
        tx.commit().await?;
        Ok(c.evidence)
    }
    pub async fn register_membership_invitation(
        &self,
        auth: &Authentication<'_>,
        raw: &[u8],
    ) -> Result<RegistrationStatus> {
        self.register_membership_invitation_at(auth, raw, now()?)
            .await
    }
    pub(super) async fn register_membership_invitation_at(
        &self,
        auth: &Authentication<'_>,
        raw: &[u8],
        time: i64,
    ) -> Result<RegistrationStatus> {
        ensure!(raw.len() == DECLARATION_BYTES, "error membership-limit");
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let c = current(&mut tx).await?;
        c.membership.authenticate(auth, false)?;
        let d = Declaration::from_record(&c.membership, raw)?;
        ensure!(d.inviter == auth.device, "error enrollment-unauthorized");
        let expiry = i64::try_from(d.expiry()).context("error enrollment-expired")?;
        let high = observe(&mut tx, time).await?;
        let stored: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT declaration FROM server_membership_invitations WHERE handle=?",
        )
        .bind(d.handle.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(stored) = stored {
            ensure!(stored == raw, "error enrollment-conflict");
        } else {
            let count: i64 =
                sqlx::query_scalar("SELECT count(*) FROM server_membership_invitations")
                    .fetch_one(&mut *tx)
                    .await?;
            ensure!(
                count < MAX_INVITATIONS as i64 && c.membership.device_count() < MAX_DEVICES,
                "error membership-limit"
            );
            ensure!(
                expiry <= high || expiry - high <= 3600,
                "error enrollment-expired"
            );
            let unfinished: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM server_membership_invitations WHERE inviter=? AND expired=0 AND admitted_sequence IS NULL)")
                .bind(d.inviter.as_slice()).fetch_one(&mut *tx).await?;
            if unfinished {
                tx.commit().await?;
                anyhow::bail!("error enrollment-unfinished");
            }
            sqlx::query("INSERT INTO server_membership_invitations(handle,inviter,declaration,expiry,expired) VALUES(?,?,?,?,?)")
                .bind(d.handle.as_slice()).bind(d.inviter.as_slice()).bind(raw).bind(expiry).bind(expiry <= high).execute(&mut *tx).await?;
        }
        let (expired, consumed): (bool, bool) = sqlx::query_as("SELECT expired,admitted_sequence IS NOT NULL FROM server_membership_invitations WHERE handle=?")
            .bind(d.handle.as_slice()).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(if consumed {
            RegistrationStatus::Consumed
        } else if expired {
            RegistrationStatus::Expired
        } else {
            RegistrationStatus::Open
        })
    }
    pub async fn post_membership_request(
        &self,
        vault: Hash,
        handle: Hash,
        request: &[u8],
    ) -> Result<()> {
        self.post_membership_request_at(vault, handle, request, now()?)
            .await
    }
    pub(super) async fn post_membership_request_at(
        &self,
        vault: Hash,
        handle: Hash,
        request: &[u8],
        time: i64,
    ) -> Result<()> {
        request_parts(request, &handle)?;
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let c = current(&mut tx).await?;
        check(vault == c.membership.genesis.context.vault_id)?;
        observe(&mut tx, time).await?;
        let (stored, expired, revoked, admitted): (Option<Vec<u8>>, bool, bool, bool) = sqlx::query_as(
            "SELECT request,expired,revoked,admitted_sequence IS NOT NULL FROM server_membership_invitations WHERE handle=?",
        )
        .bind(handle.as_slice())
        .fetch_optional(&mut *tx)
        .await?
        .context("error enrollment-unavailable")?;
        ensure!(
            !revoked && (!admitted || c.membership.members.iter().any(|m| m.admission == handle)),
            "error enrollment-revoked"
        );
        if let Some(stored) = stored {
            ensure!(stored == request, "error enrollment-request-occupied");
        } else {
            if expired {
                tx.commit().await?;
                anyhow::bail!("error enrollment-expired");
            }
            sqlx::query("UPDATE server_membership_invitations SET request=? WHERE handle=?")
                .bind(request)
                .bind(handle.as_slice())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn membership_mailbox(&self, vault: Hash, handle: Hash) -> Result<Mailbox> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let c = current(&mut tx).await?;
        check(vault == c.membership.genesis.context.vault_id)?;
        let (declaration, request, admission): (Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>) = sqlx::query_as("SELECT i.declaration,i.request,a.record FROM server_membership_invitations i LEFT JOIN server_membership_transitions a ON a.handle=i.handle WHERE i.handle=?")
            .bind(handle.as_slice()).fetch_optional(&mut *tx).await?.context("error enrollment-unavailable")?;
        if admission.is_some() {
            ensure!(
                c.membership.members.iter().any(|m| m.admission == handle),
                "error enrollment-revoked"
            );
        } else {
            let d = Declaration::from_record(&c.membership, &declaration)?;
            check(d.handle == handle)?;
        }
        tx.commit().await?;
        Ok(Mailbox {
            declaration,
            request,
            admission,
        })
    }
    /// Terminally fences a registered, unadmitted invitation of the caller so
    /// no new admission can commit. Serialized with admission; idempotent.
    /// Unknown handles allocate nothing.
    pub async fn cancel_membership_invitation(
        &self,
        auth: &Authentication<'_>,
        handle: Hash,
    ) -> Result<CancelStatus> {
        self.cancel_membership_invitation_at(auth, handle, now()?)
            .await
    }
    pub(super) async fn cancel_membership_invitation_at(
        &self,
        auth: &Authentication<'_>,
        handle: Hash,
        time: i64,
    ) -> Result<CancelStatus> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let c = current(&mut tx).await?;
        c.membership.authenticate(auth, false)?;
        let (declaration, inviter, admitted): (Vec<u8>, Vec<u8>, bool) = sqlx::query_as("SELECT declaration,inviter,admitted_sequence IS NOT NULL FROM server_membership_invitations WHERE handle=?")
            .bind(handle.as_slice())
            .fetch_optional(&mut *tx)
            .await?
            .context("error enrollment-unavailable")?;
        // The immutable stored row binds the exact declaration and its inviter.
        ensure!(
            inviter == auth.device
                && Declaration::from_record(&c.membership, &declaration)?.handle == handle,
            "error enrollment-unauthorized"
        );
        let status = if admitted {
            CancelStatus::Admitted
        } else {
            observe(&mut tx, time).await?;
            sqlx::query("UPDATE server_membership_invitations SET expired=1 WHERE handle=?")
                .bind(handle.as_slice())
                .execute(&mut *tx)
                .await?;
            CancelStatus::Cancelled
        };
        tx.commit().await?;
        Ok(status)
    }
    pub async fn admit_membership_device(
        &self,
        auth: &Authentication<'_>,
        handle: Hash,
        record: &[u8],
    ) -> Result<Vec<u8>> {
        self.admit_membership_device_at(auth, handle, record, now()?)
            .await
    }
    pub(super) async fn admit_membership_device_at(
        &self,
        auth: &Authentication<'_>,
        handle: Hash,
        record: &[u8],
        time: i64,
    ) -> Result<Vec<u8>> {
        ensure!(record.len() <= MAX_RECORD_BYTES, "error membership-limit");
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let c = current(&mut tx).await?;
        c.membership.authenticate(auth, false)?;
        type Row = (Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>, bool);
        let (declaration, request, saved, fenced): Row = sqlx::query_as("SELECT i.declaration,i.request,a.record,i.expired FROM server_membership_invitations i LEFT JOIN server_membership_transitions a ON a.handle=i.handle WHERE i.handle=?")
            .bind(handle.as_slice()).fetch_optional(&mut *tx).await?.context("error enrollment-unavailable")?;
        if let Some(saved) = saved {
            ensure!(saved == record, "error enrollment-conflict");
            observe(&mut tx, time).await?;
        } else {
            let d = Declaration::from_record(&c.membership, &declaration)?;
            ensure!(
                d.handle == handle && d.inviter == auth.device,
                "error enrollment-unauthorized"
            );
            let high = observe(&mut tx, time).await?;
            // Cancelled and clock-expired rows stay terminal for new admission.
            if fenced || high as u64 >= d.expiry() {
                tx.commit().await?;
                anyhow::bail!("error enrollment-expired");
            }
            let request = request.context("error enrollment-request-missing")?;
            let next = c.membership.append(&declaration, &request, record)?;
            append_transition(&mut tx, &c.membership, &next, Some(handle), record).await?;
            sqlx::query(
                "UPDATE server_membership_invitations SET admitted_sequence=? WHERE handle=?",
            )
            .bind(next.sequence() as i64)
            .bind(handle.as_slice())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(record.to_vec())
    }
}
#[cfg(test)]
mod tests;

impl Database {
    pub async fn membership_checkpoint_mirror(&self) -> Result<Option<(Hash, u64, Hash, Hash)>> {
        let mut conn = self.acquire_reader().await?;
        type Row = (Vec<u8>, i64, Vec<u8>, Vec<u8>);
        let row: Option<Row> = sqlx::query_as("SELECT identity,sequence,head,evidence FROM local_membership_checkpoint WHERE singleton=1").fetch_optional(&mut *conn).await?;
        row.map(|(id, seq, head, evidence)| {
            Ok((
                id.try_into()
                    .map_err(|_| anyhow::anyhow!("error membership-mirror"))?,
                u64::try_from(seq)?,
                head.try_into()
                    .map_err(|_| anyhow::anyhow!("error membership-mirror"))?,
                evidence
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("error membership-mirror"))?,
            ))
        })
        .transpose()
    }
    pub async fn mirror_membership_checkpoint(
        &self,
        identity: Hash,
        m: &Membership,
        evidence: Hash,
    ) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_peer_enrollment WHERE identity=? AND client_id=(SELECT value FROM meta WHERE key='client_id'))").bind(identity.as_slice()).fetch_one(&mut *tx).await?;
        ensure!(valid, "error membership-identity");
        type MirrorRow = (Vec<u8>, i64, Vec<u8>, Vec<u8>);
        let old: Option<MirrorRow> = sqlx::query_as("SELECT identity,sequence,head,evidence FROM local_membership_checkpoint WHERE singleton=1").fetch_optional(&mut *tx).await?;
        if let Some((id, seq, head, digest)) = old {
            ensure!(
                id == identity
                    && m.head_at(u64::try_from(seq)?).is_some_and(|h| head == h)
                    && (seq != m.sequence() as i64 || digest == evidence),
                "error membership-mirror"
            );
        }
        sqlx::query("INSERT INTO local_membership_checkpoint(singleton,identity,sequence,head,evidence) VALUES(1,?,?,?,?) ON CONFLICT(singleton) DO UPDATE SET sequence=excluded.sequence,head=excluded.head,evidence=excluded.evidence")
            .bind(identity.as_slice()).bind(m.sequence() as i64).bind(m.head().as_slice()).bind(evidence.as_slice()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}
