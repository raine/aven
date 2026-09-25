use anyhow::{Result, ensure};
use subtle::ConstantTimeEq;

use super::{
    ClaimAuthentication, ClaimRefusal, ClaimResult, Genesis, SetupAuthority, codec,
    credential_verifier,
};
use crate::db::{self, Database, begin_immediate};

const SERVER_SETUP_KEY: &str = "e2ee_server_setup";

impl Database {
    /// Admits one immutable sequence-zero claim under operator setup authority.
    /// Without `configured_setup`, the storage's unexpired issued verifier is
    /// read in this claim transaction, so a concurrent reissue either precedes
    /// the claim or finds the storage claimed.
    /// Exact authenticated retries return the original result without reissuance.
    /// This does not authorize uploads, READY, or ordinary plaintext sync.
    pub async fn admit_seed_claim(
        &self,
        request: &[u8],
        configured_setup: Option<&SetupAuthority>,
        authentication: ClaimAuthentication<'_>,
    ) -> Result<ClaimResult> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        self.admit_seed_claim_at(request, configured_setup, authentication, now)
            .await
    }

    pub(in crate::sync::seed_claim) async fn admit_seed_claim_at(
        &self,
        request: &[u8],
        configured_setup: Option<&SetupAuthority>,
        authentication: ClaimAuthentication<'_>,
        now: u64,
    ) -> Result<ClaimResult> {
        let incoming = codec::claim_record(request)?;
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let (persisted, expired) = match configured_setup {
            Some(_) => (None, None),
            None => match db::get_meta(&mut tx, SERVER_SETUP_KEY).await? {
                Some(value) => {
                    let (setup, expires_at) = parse_server_setup(&value)?;
                    if now < expires_at {
                        (Some(setup), None)
                    } else {
                        (None, Some(setup))
                    }
                }
                None => (None, None),
            },
        };
        let configured_setup = configured_setup.or(persisted.as_ref());
        let genesis_only: Option<bool> =
            sqlx::query_scalar("SELECT genesis_only FROM server_seed_claim WHERE singleton = 1")
                .fetch_optional(&mut *tx)
                .await?;
        ensure!(genesis_only != Some(false), ClaimRefusal::Retired);
        let stored: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT genesis FROM server_seed_claim WHERE singleton = 1")
                .fetch_optional(&mut *tx)
                .await?;
        let genesis = Genesis::from_record(stored.as_deref().unwrap_or(incoming))?;
        // Only the exact secret of the expired verifier learns that it
        // expired, which tells a guesser nothing a live verifier wouldn't.
        let mut expired_secret = false;
        let authorized = match authentication {
            ClaimAuthentication::SetupSecret(secret) => {
                expired_secret = expired
                    .as_ref()
                    .is_some_and(|setup| setup.authorizes(genesis.setup, secret));
                configured_setup.is_some_and(|setup| setup.authorizes(genesis.setup, secret))
            }
            ClaimAuthentication::SeedBearer(token) => {
                stored.is_some()
                    && bool::from(
                        credential_verifier(genesis.context.vault_id, genesis.device, token)
                            .ct_eq(&genesis.verifier),
                    )
            }
        };
        ensure!(
            authorized || !expired_secret || stored.is_some(),
            ClaimRefusal::Expired
        );
        ensure!(
            authorized,
            ClaimRefusal::Unauthorized {
                claimed: stored.is_some()
            }
        );
        if let Some(stored) = stored {
            ensure!(stored == incoming, ClaimRefusal::Conflict);
        } else {
            sqlx::query("INSERT INTO server_seed_claim(singleton, genesis) VALUES (1, ?)")
                .bind(genesis.record().as_slice())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(ClaimResult::from_genesis(&genesis))
    }

    /// Whether encrypted server storage already has its immutable seed claim.
    pub async fn e2ee_server_is_claimed(&self) -> Result<bool> {
        let mut conn = self.acquire_reader().await?;
        Ok(
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM server_seed_claim WHERE singleton = 1)",
            )
            .fetch_one(&mut *conn)
            .await?,
        )
    }

    /// Nonsecret loss-detection pin. This is never a substitute for host authority.
    pub async fn local_seed_genesis_commitment(&self) -> Result<Option<[u8; 32]>> {
        let mut conn = self.acquire_reader().await?;
        let value: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT commitment FROM local_seed_genesis_pin WHERE singleton = 1")
                .fetch_optional(&mut *conn)
                .await?;
        value
            .map(|bytes| {
                bytes
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("error seed-claim-local-pin-corrupt"))
            })
            .transpose()
    }

    /// Hosts call this only after protected storage has committed exact authority.
    /// An old or replaced database may acquire a pin but cannot change one.
    pub async fn pin_local_seed_genesis(&self, genesis: &Genesis) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let commitment = genesis.commitment();
        sqlx::query(
            "INSERT OR IGNORE INTO local_seed_genesis_pin(singleton, commitment) VALUES (1, ?)",
        )
        .bind(commitment.as_slice())
        .execute(&mut *tx)
        .await?;
        let stored: Vec<u8> =
            sqlx::query_scalar("SELECT commitment FROM local_seed_genesis_pin WHERE singleton = 1")
                .fetch_one(&mut *tx)
                .await?;
        ensure!(stored == commitment, "error seed-claim-local-pin-conflict");
        tx.commit().await?;
        Ok(())
    }

    /// Records that a fenced setup cannot resume against its server.
    pub async fn mark_local_seed_setup_refused(&self, reason: &str) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        crate::db::set_meta(&mut conn, "e2ee_setup_refused", reason).await
    }

    /// Removes a provisional seed pin after a claim was definitely rejected.
    /// Once source authority or publication state exists, the installation is
    /// fenced and this operation refuses to undo it.
    pub async fn rollback_local_seed_genesis(&self, expected: [u8; 32]) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let fenced: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM local_seed_source)
                 OR EXISTS(SELECT 1 FROM local_seed_publication_intent)
                 OR EXISTS(SELECT 1 FROM local_shared_capture_journal)",
        )
        .fetch_one(&mut *tx)
        .await?;
        ensure!(!fenced, "error seed-claim-rollback-fenced");
        let stored: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT commitment FROM local_seed_genesis_pin WHERE singleton = 1")
                .fetch_optional(&mut *tx)
                .await?;
        ensure!(
            stored
                .as_deref()
                .is_none_or(|value| value == expected.as_slice()),
            "error seed-claim-rollback-conflict"
        );
        sqlx::query("DELETE FROM local_seed_genesis_pin WHERE singleton = 1")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Issues operator setup authority for `secret` on encrypted server
    /// storage and returns its setup ID. An existing ID is kept, even after
    /// expiry, so a device whose genesis already binds it can claim with the
    /// reissued secret; the replaced verifier refuses earlier secrets. Claimed
    /// storage and storage holding any task history cannot take a new verifier.
    pub async fn issue_e2ee_server_setup(
        &self,
        secret: &super::Secret,
        fresh_id: [u8; 32],
        expires_at: u64,
    ) -> Result<[u8; 32]> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let claimed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM server_seed_claim)")
            .fetch_one(&mut *tx)
            .await?;
        ensure!(!claimed, "error e2ee-server-already-claimed");
        let history: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM changes)")
            .fetch_one(&mut *tx)
            .await?;
        ensure!(!history, "error e2ee-server-storage-not-empty");
        let id = match db::get_meta(&mut tx, SERVER_SETUP_KEY).await? {
            Some(value) => parse_server_setup(&value)?.0.id,
            None => fresh_id,
        };
        let value = format!(
            "{}:{}:{expires_at}",
            hex::encode(id),
            hex::encode(SetupAuthority::verifier(id, secret))
        );
        db::set_meta(&mut tx, SERVER_SETUP_KEY, &value).await?;
        tx.commit().await?;
        Ok(id)
    }

    /// The configured setup verifier while it is unexpired at `now` (Unix seconds).
    pub async fn e2ee_server_setup(&self, now: u64) -> Result<Option<SetupAuthority>> {
        let Some(value) = self.meta(SERVER_SETUP_KEY).await? else {
            return Ok(None);
        };
        let (setup, expires_at) = parse_server_setup(&value)?;
        Ok((now < expires_at).then_some(setup))
    }

    /// True once storage was prepared for, or claimed by, an encrypted vault.
    pub async fn is_e2ee_server_storage(&self) -> Result<bool> {
        if self.meta(SERVER_SETUP_KEY).await?.is_some() {
            return Ok(true);
        }
        let mut conn = self.acquire_reader().await?;
        Ok(
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM server_seed_claim)")
                .fetch_one(&mut *conn)
                .await?,
        )
    }

    /// True when storage holds change rows. Encrypted server storage never
    /// does, so server launch refuses such storage as unsupported.
    pub async fn has_change_history(&self) -> Result<bool> {
        let mut conn = self.acquire_reader().await?;
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM changes)")
            .fetch_one(&mut *conn)
            .await?)
    }
}

fn parse_server_setup(value: &str) -> Result<(SetupAuthority, u64)> {
    let corrupt = || anyhow::anyhow!("error e2ee-server-setup-corrupt");
    let mut parts = value.split(':');
    let (Some(id), Some(verifier), Some(expires_at), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(corrupt());
    };
    let decode = |text: &str| -> Result<[u8; 32]> {
        hex::decode(text)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(corrupt)
    };
    let expires_at = expires_at.parse().map_err(|_| corrupt())?;
    Ok((
        SetupAuthority::from_verifier(decode(id)?, decode(verifier)?),
        expires_at,
    ))
}
