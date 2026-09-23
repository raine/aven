//! Bounded signed history, invitation outcomes and vault-clock transactions.
use super::super::peer::{Authentication, RegistrationStatus, request_parts};
use super::*;
use crate::db::{Database, begin_immediate};
use serde::{Deserialize, Serialize};
use sqlx::SqliteConnection;

pub const MAX_INVITATIONS: usize = 128;
pub const MAX_CANDIDATES: usize = 8;
pub const MAX_EVIDENCE_JSON_BYTES: usize = 4 * MAX_CHAIN_BYTES + 4096;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRecord {
    #[serde(deserialize_with = "declaration_bytes")]
    pub declaration: Vec<u8>,
    #[serde(deserialize_with = "request_bytes")]
    pub request: Vec<u8>,
    #[serde(deserialize_with = "record_bytes")]
    pub record: Vec<u8>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    #[serde(deserialize_with = "genesis_bytes")]
    pub genesis: Vec<u8>,
    #[serde(deserialize_with = "publication_bytes")]
    pub publication: Vec<u8>,
    #[serde(deserialize_with = "descriptor_bytes")]
    pub descriptor: Vec<u8>,
    #[serde(deserialize_with = "records")]
    pub admissions: Vec<EvidenceRecord>,
}
fn bounded<'de, D, T, const N: usize>(d: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Visitor<T, const N: usize>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const N: usize> serde::de::Visitor<'de> for Visitor<T, N> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("bounded membership evidence")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            if seq.size_hint().is_some_and(|n| n > N) {
                return Err(serde::de::Error::custom("membership-limit"));
            }
            let mut values = Vec::new();
            while values.len() < N {
                let Some(value) = seq.next_element()? else {
                    return Ok(values);
                };
                values.push(value);
            }
            if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                return Err(serde::de::Error::custom("membership-limit"));
            }
            Ok(values)
        }
    }
    d.deserialize_seq(Visitor::<T, N>(std::marker::PhantomData))
}
fn record_bytes<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<u8>, D::Error> {
    bounded::<D, u8, { 1403 + 164 * MAX_DEVICES }>(d)
}
// The aggregate of every independently bounded field fits before allocation.
const _: () = assert!(
    (MAX_DEVICES - 1) * (1403 + 164 * MAX_DEVICES + DECLARATION_BYTES + REQUEST_BYTES)
        + GENESIS_BYTES
        + PUBLICATION_BYTES
        + 1024
        <= MAX_CHAIN_BYTES
);
fn declaration_bytes<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Vec<u8>, D::Error> {
    bounded::<D, u8, DECLARATION_BYTES>(d)
}
fn request_bytes<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<u8>, D::Error> {
    bounded::<D, u8, REQUEST_BYTES>(d)
}
fn genesis_bytes<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<u8>, D::Error> {
    bounded::<D, u8, GENESIS_BYTES>(d)
}
fn publication_bytes<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Vec<u8>, D::Error> {
    bounded::<D, u8, PUBLICATION_BYTES>(d)
}
fn descriptor_bytes<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Vec<u8>, D::Error> {
    bounded::<D, u8, 1024>(d)
}
fn records<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Vec<EvidenceRecord>, D::Error> {
    bounded::<D, EvidenceRecord, { MAX_DEVICES - 1 }>(d)
}
impl Evidence {
    /// Verify the complete chain while retaining the exact original enrollment.
    pub fn enrollment(&self, peer: &Joiner, outcome: Hash) -> Result<VerifiedEnrollment> {
        let current = self.verify()?;
        let g = Genesis::from_record(&self.genesis)?;
        let mut before = Membership::from_publication(&g, &self.descriptor, &self.publication)?;
        for a in &self.admissions {
            if hash(&a.record) == outcome {
                let verified = peer.verify_enrollment(&before, &a.declaration, &a.record)?;
                ensure!(
                    current.extends(verified.membership()),
                    "error membership-fork"
                );
                return Ok(verified);
            }
            before = before.append(&a.declaration, &a.request, &a.record)?;
        }
        anyhow::bail!("error enrollment-outcome-missing")
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= MAX_EVIDENCE_JSON_BYTES,
            "error membership-limit"
        );
        let evidence: Self = serde_json::from_slice(bytes)
            .map_err(|_| anyhow::anyhow!("error membership-evidence"))?;
        evidence.verify()?;
        Ok(evidence)
    }
    pub fn verify(&self) -> Result<Membership> {
        ensure!(
            self.admissions.len() < MAX_DEVICES,
            "error membership-limit"
        );
        let mut size = self
            .genesis
            .len()
            .checked_add(self.publication.len())
            .and_then(|n| n.checked_add(self.descriptor.len()))
            .context("error membership-limit")?;
        for a in &self.admissions {
            ensure!(
                a.declaration.len() == DECLARATION_BYTES
                    && a.request.len() == REQUEST_BYTES
                    && a.record.len() <= MAX_RECORD_BYTES,
                "error membership-limit"
            );
            size = size
                .checked_add(a.declaration.len())
                .and_then(|n| n.checked_add(a.request.len()))
                .and_then(|n| n.checked_add(a.record.len()))
                .context("error membership-limit")?;
        }
        ensure!(size <= MAX_CHAIN_BYTES, "error membership-limit");
        let genesis = Genesis::from_record(&self.genesis)?;
        let mut m = Membership::from_publication(&genesis, &self.descriptor, &self.publication)?;
        for a in &self.admissions {
            m = m.append(&a.declaration, &a.request, &a.record)?;
        }
        Ok(m)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mailbox {
    #[serde(deserialize_with = "declaration_bytes")]
    pub declaration: Vec<u8>,
    pub request: Option<Vec<u8>>,
    pub admission: Option<Vec<u8>>,
}
pub(crate) struct Current {
    pub membership: Membership,
    pub evidence: Evidence,
}
/// Bound stored byte lengths before materializing any attacker-influenced blobs.
pub(crate) async fn current(conn: &mut SqliteConnection) -> Result<Current> {
    let old: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM server_peer_invitation)")
        .fetch_one(&mut *conn)
        .await?;
    ensure!(!old, "error enrollment-store-unsupported");
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
    let (count, total, largest): (i64, i64, i64) = sqlx::query_as("SELECT count(*),coalesce(sum(length(record)),0),coalesce(max(length(record)),0) FROM server_membership_admissions")
        .fetch_one(&mut *conn).await?;
    ensure!(
        (0..MAX_DEVICES as i64).contains(&count)
            && largest <= MAX_RECORD_BYTES as i64
            && total >= 0
            && g + p + d + total + count * (DECLARATION_BYTES + REQUEST_BYTES) as i64
                <= MAX_CHAIN_BYTES as i64,
        "error membership-limit"
    );
    let (genesis, publication, descriptor): (Vec<u8>, Vec<u8>, Vec<u8>) = sqlx::query_as("SELECT s.genesis,p.signed_record,p.descriptor FROM server_seed_claim s JOIN server_bootstrap_publication p ON p.singleton=s.singleton WHERE s.singleton=1")
        .fetch_one(&mut *conn).await?;
    type Row = (i64, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, i64, i64);
    let rows: Vec<Row> = sqlx::query_as("SELECT a.sequence,a.handle,a.record,i.declaration,i.request,i.inviter,i.expiry,i.admitted_sequence FROM server_membership_admissions a JOIN server_membership_invitations i ON i.handle=a.handle ORDER BY a.sequence")
        .fetch_all(&mut *conn).await?;
    ensure!(
        rows.len() == count as usize,
        "error membership-history-missing"
    );
    let mut evidence = Evidence {
        genesis,
        publication,
        descriptor,
        admissions: Vec::with_capacity(rows.len()),
    };
    let mut membership = evidence.verify()?;
    for (seq, handle, record, declaration, request, inviter, expiry, admitted) in rows {
        ensure!(
            seq == membership.sequence() as i64 + 1 && admitted == seq,
            "error membership-history-missing"
        );
        let dec = Declaration::from_record(&membership, &declaration)?;
        ensure!(
            handle == dec.handle
                && inviter == dec.inviter
                && expiry >= 0
                && expiry as u64 == dec.expiry(),
            "error membership-storage"
        );
        membership = membership.append(&declaration, &request, &record)?;
        evidence.admissions.push(EvidenceRecord {
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
    let orphans: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM server_membership_invitations i WHERE admitted_sequence IS NOT NULL AND NOT EXISTS(SELECT 1 FROM server_membership_admissions a WHERE a.handle=i.handle AND a.sequence=i.admitted_sequence))")
        .fetch_one(&mut *conn).await?;
    ensure!(!orphans, "error membership-history-missing");
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
        let (stored, expired): (Option<Vec<u8>>, bool) = sqlx::query_as(
            "SELECT request,expired FROM server_membership_invitations WHERE handle=?",
        )
        .bind(handle.as_slice())
        .fetch_optional(&mut *tx)
        .await?
        .context("error enrollment-unavailable")?;
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
        let (declaration, request, admission): (Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>) = sqlx::query_as("SELECT i.declaration,i.request,a.record FROM server_membership_invitations i LEFT JOIN server_membership_admissions a ON a.handle=i.handle WHERE i.handle=?")
            .bind(handle.as_slice()).fetch_optional(&mut *tx).await?.context("error enrollment-unavailable")?;
        let d = Declaration::from_record(&c.membership, &declaration)?;
        check(d.handle == handle)?;
        tx.commit().await?;
        Ok(Mailbox {
            declaration,
            request,
            admission,
        })
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
        let (declaration, request, saved): (Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>) = sqlx::query_as("SELECT i.declaration,i.request,a.record FROM server_membership_invitations i LEFT JOIN server_membership_admissions a ON a.handle=i.handle WHERE i.handle=?")
            .bind(handle.as_slice()).fetch_optional(&mut *tx).await?.context("error enrollment-unavailable")?;
        let d = Declaration::from_record(&c.membership, &declaration)?;
        ensure!(
            d.handle == handle && d.inviter == auth.device,
            "error enrollment-unauthorized"
        );
        if let Some(saved) = saved {
            ensure!(saved == record, "error enrollment-conflict");
            observe(&mut tx, time).await?;
        } else {
            let high = observe(&mut tx, time).await?;
            if high as u64 >= d.expiry() {
                tx.commit().await?;
                anyhow::bail!("error enrollment-expired");
            }
            let request = request.context("error enrollment-request-missing")?;
            let next = c.membership.append(&declaration, &request, record)?;
            sqlx::query(
                "INSERT INTO server_membership_admissions(sequence,handle,record) VALUES(?,?,?)",
            )
            .bind(next.sequence() as i64)
            .bind(handle.as_slice())
            .bind(record)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE server_membership_invitations SET admitted_sequence=? WHERE handle=?",
            )
            .bind(next.sequence() as i64)
            .bind(handle.as_slice())
            .execute(&mut *tx)
            .await?;
            let changed = sqlx::query("UPDATE server_e2ee_membership_head SET sequence=?,commitment=? WHERE singleton=1 AND sequence=? AND commitment=?")
                .bind(next.sequence() as i64).bind(next.head().as_slice()).bind(c.membership.sequence() as i64).bind(c.membership.head().as_slice()).execute(&mut *tx).await?;
            ensure!(
                changed.rows_affected() == 1,
                "error enrollment-context-stale"
            );
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
