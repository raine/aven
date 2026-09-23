use anyhow::{Result, ensure};
use subtle::ConstantTimeEq;

use super::{
    ClaimAuthentication, ClaimResult, Genesis, SetupAuthority, codec, credential_verifier,
};
use crate::db::{Database, begin_immediate};

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
}
