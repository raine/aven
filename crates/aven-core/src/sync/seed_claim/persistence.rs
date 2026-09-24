use anyhow::{Result, ensure};
use subtle::ConstantTimeEq;

use super::{
    ClaimAuthentication, ClaimResult, Genesis, SetupAuthority, codec, credential_verifier,
};
use crate::db::{self, Database, begin_immediate};

const SERVER_SETUP_KEY: &str = "e2ee_server_setup";

impl Database {
    /// Admits one immutable sequence-zero claim under operator setup authority.
    /// Exact authenticated retries return the original result without reissuance.
    /// This does not authorize uploads, READY, or ordinary plaintext sync.
    pub async fn admit_seed_claim(
        &self,
        request: &[u8],
        configured_setup: Option<&SetupAuthority>,
        authentication: ClaimAuthentication<'_>,
    ) -> Result<ClaimResult> {
        let incoming = codec::claim_record(request)?;
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let genesis_only: Option<bool> =
            sqlx::query_scalar("SELECT genesis_only FROM server_seed_claim WHERE singleton = 1")
                .fetch_optional(&mut *tx)
                .await?;
        ensure!(genesis_only != Some(false), "error seed-claim-retired");
        let stored: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT genesis FROM server_seed_claim WHERE singleton = 1")
                .fetch_optional(&mut *tx)
                .await?;
        let genesis = Genesis::from_record(stored.as_deref().unwrap_or(incoming))?;
        let authorized = match authentication {
            ClaimAuthentication::SetupSecret(secret) => {
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
        ensure!(authorized, "error seed-claim-unauthorized");
        if let Some(stored) = stored {
            ensure!(stored == incoming, "error seed-claim-conflict");
        } else {
            sqlx::query("INSERT INTO server_seed_claim(singleton, genesis) VALUES (1, ?)")
                .bind(genesis.record().as_slice())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(ClaimResult::from_genesis(&genesis))
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

    /// Replaces the operator setup verifier of encrypted server storage. Claimed
    /// storage and storage holding any task history cannot take a new verifier.
    pub async fn issue_e2ee_server_setup(
        &self,
        setup: &SetupAuthority,
        expires_at: u64,
    ) -> Result<()> {
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
        let value = format!(
            "{}:{}:{expires_at}",
            hex::encode(setup.id),
            hex::encode(setup.verifier)
        );
        db::set_meta(&mut tx, SERVER_SETUP_KEY, &value).await?;
        tx.commit().await?;
        Ok(())
    }

    /// The configured setup verifier while it is unexpired at `now` (Unix seconds).
    pub async fn e2ee_server_setup(&self, now: u64) -> Result<Option<SetupAuthority>> {
        let Some(value) = self.meta(SERVER_SETUP_KEY).await? else {
            return Ok(None);
        };
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
        let expires_at: u64 = expires_at.parse().map_err(|_| corrupt())?;
        let setup = SetupAuthority::from_verifier(decode(id)?, decode(verifier)?);
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
}
