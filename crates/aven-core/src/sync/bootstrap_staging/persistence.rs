use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};
use sqlx::SqliteConnection;

use super::*;
use crate::db::{Database, begin_immediate};
use crate::sync::bootstrap_format::staging::{ArtifactView, DeclarationView};
use crate::sync::seed_claim::Genesis;

const CATALOGS: [Component; 3] = [
    Component::DataCatalog,
    Component::PrefixCatalog,
    Component::ImageCatalog,
];

type CandidateRow = (
    Option<Vec<u8>>,
    bool,
    i64,
    i64,
    i64,
    i64,
    Option<i64>,
    Option<i64>,
);

struct Candidate {
    descriptor: Option<Vec<u8>>,
    canceled: bool,
    epoch: i64,
    expires: i64,
    budget: Budget,
    failure: Option<CatalogFailure>,
}

impl Candidate {
    fn declaration(&self) -> Result<DeclarationView> {
        ensure!(!self.canceled, "error bootstrap-canceled");
        Ok(DeclarationView::decode(
            self.descriptor
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("error bootstrap-storage-invalid"))?,
        )?)
    }

    fn commitment(&self) -> Result<[u8; 32]> {
        self.declaration()?;
        Ok(hash(self.descriptor.as_deref().unwrap()))
    }

    fn check(&self, commitment: [u8; 32], epoch: Option<u64>) -> Result<()> {
        ensure!(
            self.commitment()? == commitment,
            "error bootstrap-descriptor-conflict"
        );
        if let Some(epoch) = epoch {
            ensure!(
                u64::try_from(self.epoch)? == epoch,
                "error bootstrap-stale-epoch"
            );
        }
        Ok(())
    }
}

fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn now() -> Result<i64> {
    Ok(i64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    )?)
}

async fn authorize(conn: &mut SqliteConnection, auth: &Authentication<'_>) -> Result<Genesis> {
    let row: Option<(Vec<u8>, bool)> =
        sqlx::query_as("SELECT genesis, genesis_only FROM server_seed_claim WHERE singleton = 1")
            .fetch_optional(&mut *conn)
            .await?;
    let (record, genesis_only) =
        row.ok_or_else(|| anyhow::anyhow!("error bootstrap-unauthorized"))?;
    ensure!(genesis_only, "error bootstrap-unauthorized");
    let genesis = Genesis::from_record(&record)?;
    ensure!(
        genesis.authorizes_bearer(auth.bearer)
            && genesis.context().vault_id == auth.vault_id
            && genesis.commitment() == auth.genesis_commitment,
        "error bootstrap-unauthorized"
    );
    Ok(genesis)
}

async fn candidate(conn: &mut SqliteConnection, id: &[u8; 32]) -> Result<Option<Candidate>> {
    let row: Option<CandidateRow> = sqlx::query_as(
        "SELECT descriptor, canceled, epoch, expires_at, byte_budget, chunk_budget, catalog_failure, failure_reason
         FROM server_bootstrap_candidates WHERE bootstrap = ?",
    )
    .bind(id.as_slice())
    .fetch_optional(&mut *conn)
    .await?;
    row.map(
        |(descriptor, canceled, epoch, expires, bytes, chunks, failure, reason)| {
            Ok(Candidate {
                descriptor,
                canceled,
                epoch,
                expires,
                budget: Budget {
                    bytes: u64::try_from(bytes)?,
                    chunks: u64::try_from(chunks)?,
                },
                failure: match (failure, reason) {
                    (Some(class @ 0..=2), Some(reason @ 0..=1)) => Some(CatalogFailure {
                        component: CATALOGS[class as usize],
                        reason: if reason == 0 {
                            CatalogFailureReason::Invalid
                        } else {
                            CatalogFailureReason::ResourceLimit
                        },
                    }),
                    (None, None) => None,
                    _ => anyhow::bail!("error bootstrap-storage-invalid"),
                },
            })
        },
    )
    .transpose()
}

async fn required(conn: &mut SqliteConnection, id: &[u8; 32]) -> Result<Candidate> {
    candidate(conn, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("error bootstrap-undeclared"))
}

async fn capacity(conn: &mut SqliteConnection) -> Result<()> {
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM server_bootstrap_candidates")
        .fetch_one(&mut *conn)
        .await?;
    ensure!(count < MAX_CANDIDATES, "error bootstrap-candidate-limit");
    Ok(())
}

struct Layout {
    components: Vec<(Component, Vec<u64>)>,
    artifacts: Vec<ArtifactView>,
}

impl Layout {
    fn check_budget(&self, budget: Budget) -> Result<()> {
        let mut bytes = 0_u64;
        let mut chunks = 0_u64;
        for (_, lengths) in &self.components {
            for length in lengths {
                bytes = bytes
                    .checked_add(*length)
                    .ok_or_else(|| anyhow::anyhow!("error bootstrap-budget"))?;
                chunks = chunks
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("error bootstrap-budget"))?;
            }
        }
        ensure!(
            bytes <= budget.bytes && chunks <= budget.chunks,
            "error bootstrap-budget"
        );
        Ok(())
    }
}

async fn records(
    conn: &mut SqliteConnection,
    id: &[u8; 32],
    component: Component,
) -> Result<Vec<Vec<u8>>> {
    Ok(sqlx::query_scalar(
        "SELECT bytes FROM server_bootstrap_chunks WHERE bootstrap = ? AND component = ? ORDER BY chunk_index",
    ).bind(id.as_slice()).bind(component.key()).fetch_all(&mut *conn).await?)
}

async fn layout(conn: &mut SqliteConnection, id: &[u8; 32], d: &DeclarationView) -> Result<Layout> {
    let mut components = Vec::new();
    let mut artifacts = d.artifacts(None);
    for (class, component) in CATALOGS.into_iter().enumerate() {
        let lengths = d.catalog_lengths(class)?;
        let verified: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM server_bootstrap_chunks WHERE bootstrap = ? AND component = ? AND verified = 1",
        ).bind(id.as_slice()).bind(component.key()).fetch_one(&mut *conn).await?;
        if verified != 0 {
            ensure!(
                usize::try_from(verified)? == lengths.len(),
                "error bootstrap-storage-invalid"
            );
            let bytes = records(conn, id, component).await?.concat();
            let catalog = d.catalog(class, &bytes)?;
            artifacts.extend(d.artifacts(Some(&catalog)));
        }
        components.push((component, lengths));
    }
    components.extend(artifacts.iter().map(|a| (a.component, a.lengths())));
    Ok(Layout {
        components,
        artifacts,
    })
}

async fn status(conn: &mut SqliteConnection, id: &[u8; 32]) -> Result<Status> {
    let Some(c) = candidate(conn, id).await? else {
        return Ok(Status::Missing);
    };
    if c.canceled {
        return Ok(Status::Canceled);
    }
    let d = c.declaration()?;
    let layout = layout(conn, id, &d).await?;
    layout.check_budget(c.budget)?;
    let mut components = Vec::new();
    for (component, lengths) in layout.components {
        let rows: Vec<(i64, bool)> = sqlx::query_as(
            "SELECT chunk_index, verified FROM server_bootstrap_chunks WHERE bootstrap = ? AND component = ? ORDER BY chunk_index",
        ).bind(id.as_slice()).bind(component.key()).fetch_all(&mut *conn).await?;
        let mut chunks = vec![Presence::Missing; lengths.len()];
        for (index, verified) in rows {
            let slot = chunks
                .get_mut(usize::try_from(index)?)
                .ok_or_else(|| anyhow::anyhow!("error bootstrap-storage-invalid"))?;
            *slot = if verified {
                Presence::Verified
            } else {
                Presence::Quarantined
            };
        }
        components.push(ComponentStatus { component, chunks });
    }
    Ok(Status::Staging(StagingStatus {
        descriptor_commitment: c.commitment()?,
        stream_id: d.binding().stream,
        epoch: u64::try_from(c.epoch)?,
        expires_at: c.expires,
        budget: c.budget,
        catalog_failure: c.failure,
        components,
    }))
}

async fn advance(
    conn: &mut SqliteConnection,
    id: &[u8; 32],
    c: &Candidate,
    reclaim: Reclaim,
) -> Result<()> {
    let epoch = c
        .epoch
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("error bootstrap-epoch-exhausted"))?;
    sqlx::query("DELETE FROM server_bootstrap_chunks WHERE bootstrap = ? AND (? OR verified = 0)")
        .bind(id.as_slice())
        .bind(reclaim == Reclaim::All)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "UPDATE server_bootstrap_candidates SET epoch = ?, expires_at = 0 WHERE bootstrap = ?",
    )
    .bind(epoch)
    .bind(id.as_slice())
    .execute(&mut *conn)
    .await?;
    Ok(())
}

impl Database {
    /// Authenticated declaration also ensures the initial staging reservation.
    /// Descriptor and budgets are immutable. Expired retries resume retained bytes.
    pub async fn declare_bootstrap_staging(
        &self,
        auth: &Authentication<'_>,
        descriptor: &[u8],
        budget: Budget,
    ) -> Result<StagingStatus> {
        ensure!(descriptor.len() <= 1024, "error bootstrap-descriptor-limit");
        ensure!(
            budget.bytes > 0
                && budget.bytes <= MAX_STORAGE_BYTES
                && budget.chunks > 0
                && budget.chunks <= MAX_CHUNKS,
            "error bootstrap-budget"
        );
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let genesis = authorize(&mut tx, auth).await?;
        let d = DeclarationView::decode(descriptor)?;
        let binding = d.binding();
        let id = binding.bootstrap;
        ensure!(
            binding.vault == genesis.context().vault_id
                && binding.generation == genesis.context().generation_id
                && binding.membership == genesis.commitment(),
            "error bootstrap-context"
        );
        if let Some(c) = candidate(&mut tx, &id).await? {
            c.check(hash(descriptor), None)?;
            ensure!(
                c.descriptor.as_deref() == Some(descriptor) && c.budget == budget,
                "error bootstrap-descriptor-conflict"
            );
        } else {
            capacity(&mut tx).await?;
            let active: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM server_bootstrap_candidates WHERE canceled = 0)",
            )
            .fetch_one(&mut *tx)
            .await?;
            ensure!(!active, "error bootstrap-active-candidate");
            layout(&mut tx, &id, &d).await?.check_budget(budget)?;
            sqlx::query("INSERT INTO server_bootstrap_candidates(bootstrap, descriptor, canceled, epoch, expires_at, byte_budget, chunk_budget) VALUES (?, ?, 0, 1, ?, ?, ?)")
                .bind(id.as_slice()).bind(descriptor).bind(now()?.checked_add(STAGING_TTL_SECONDS).ok_or_else(|| anyhow::anyhow!("error bootstrap-clock"))?)
                .bind(i64::try_from(budget.bytes)?).bind(i64::try_from(budget.chunks)?)
                .execute(&mut *tx).await?;
        }
        let result = ensure_staging(&mut tx, &id, hash(descriptor)).await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Read-only, including for missing/expired candidates. No reservation renewal.
    /// Callers compare the returned commitment with their frozen local descriptor.
    /// Missing is an observation, never a terminal cancellation fence.
    pub async fn bootstrap_staging_status(
        &self,
        auth: &Authentication<'_>,
        bootstrap_id: [u8; 32],
    ) -> Result<Status> {
        let mut conn = self.acquire_reader().await?;
        use sqlx::Connection;
        let mut tx = conn.begin().await?;
        authorize(&mut tx, auth).await?;
        let result = status(&mut tx, &bootstrap_id).await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn ensure_bootstrap_staging(
        &self,
        auth: &Authentication<'_>,
        bootstrap_id: [u8; 32],
        descriptor_commitment: [u8; 32],
    ) -> Result<StagingStatus> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        authorize(&mut tx, auth).await?;
        let result = ensure_staging(&mut tx, &bootstrap_id, descriptor_commitment).await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Terminal even before declaration. Canceled IDs consume the finite lifetime
    /// candidate budget and are never silently forgotten or reopened.
    pub async fn cancel_bootstrap_staging(
        &self,
        auth: &Authentication<'_>,
        bootstrap_id: [u8; 32],
    ) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        authorize(&mut tx, auth).await?;
        if candidate(&mut tx, &bootstrap_id).await?.is_none() {
            capacity(&mut tx).await?;
            sqlx::query("INSERT INTO server_bootstrap_candidates(bootstrap, canceled, epoch, expires_at, byte_budget, chunk_budget) VALUES (?, 1, 1, 0, 0, 0)")
                .bind(bootstrap_id.as_slice()).execute(&mut *tx).await?;
        } else {
            sqlx::query("DELETE FROM server_bootstrap_chunks WHERE bootstrap = ?")
                .bind(bootstrap_id.as_slice())
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE server_bootstrap_candidates SET canceled = 1, descriptor = NULL, expires_at = 0, byte_budget = 0, chunk_budget = 0, catalog_failure = NULL, failure_reason = NULL WHERE bootstrap = ?")
                .bind(bootstrap_id.as_slice()).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Authenticated reclamation serializes deletion and epoch fencing with PUT.
    /// It cannot cancel the candidate or affect a resumed epoch using an old token.
    pub async fn reclaim_bootstrap_staging(
        &self,
        auth: &Authentication<'_>,
        bootstrap_id: [u8; 32],
        descriptor_commitment: [u8; 32],
        epoch: u64,
        reclaim: Reclaim,
    ) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        authorize(&mut tx, auth).await?;
        let c = required(&mut tx, &bootstrap_id).await?;
        c.check(descriptor_commitment, Some(epoch))?;
        advance(&mut tx, &bootstrap_id, &c, reclaim).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn put_bootstrap_chunk(
        &self,
        auth: &Authentication<'_>,
        request: PutChunk<'_>,
    ) -> Result<PutOutcome> {
        ensure!(
            request.bytes.len() <= MAX_REQUEST_BYTES,
            "error bootstrap-request-limit"
        );
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        authorize(&mut tx, auth).await?;
        let id = &request.bootstrap_id;
        let c = required(&mut tx, id).await?;
        c.check(request.descriptor_commitment, Some(request.epoch))?;
        ensure!(c.expires > now()?, "error bootstrap-staging-expired");
        let d = c.declaration()?;
        let layout = layout(&mut tx, id, &d).await?;
        layout.check_budget(c.budget)?;
        let lengths = &layout
            .components
            .iter()
            .find(|(component, _)| *component == request.component)
            .ok_or_else(|| anyhow::anyhow!("error bootstrap-catalog-required"))?
            .1;
        let index = usize::try_from(request.index)?;
        ensure!(
            lengths.get(index) == Some(&(request.bytes.len() as u64)),
            "error bootstrap-chunk-shape"
        );
        let existing: Option<(Vec<u8>, bool)> = sqlx::query_as("SELECT bytes, verified FROM server_bootstrap_chunks WHERE bootstrap = ? AND component = ? AND chunk_index = ?")
            .bind(id.as_slice()).bind(request.component.key()).bind(i64::try_from(index)?)
            .fetch_optional(&mut *tx).await?;
        if let Some((bytes, verified)) = existing {
            ensure!(bytes == request.bytes, "error bootstrap-chunk-conflict");
            tx.commit().await?;
            return Ok(if verified {
                PutOutcome::Verified
            } else {
                PutOutcome::Quarantined
            });
        }
        let catalog = request.component.catalog();
        if catalog.is_none() {
            layout
                .artifacts
                .iter()
                .find(|a| a.component == request.component)
                .ok_or_else(|| anyhow::anyhow!("error bootstrap-catalog-required"))?
                .verify_chunk(index, request.bytes)?;
        }
        let (bytes, chunks): (i64, i64) = sqlx::query_as("SELECT coalesce(sum(length(bytes)), 0), count(*) FROM server_bootstrap_chunks WHERE bootstrap = ?")
            .bind(id.as_slice()).fetch_one(&mut *tx).await?;
        ensure!(
            (bytes as u64)
                .checked_add(request.bytes.len() as u64)
                .is_some_and(|n| n <= c.budget.bytes)
                && (chunks as u64)
                    .checked_add(1)
                    .is_some_and(|n| n <= c.budget.chunks),
            "error bootstrap-budget"
        );
        sqlx::query("INSERT INTO server_bootstrap_chunks(bootstrap, component, chunk_index, verified, bytes) VALUES (?, ?, ?, ?, ?)")
            .bind(id.as_slice()).bind(request.component.key()).bind(i64::try_from(index)?)
            .bind(catalog.is_none()).bind(request.bytes).execute(&mut *tx).await?;
        let stored: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM server_bootstrap_chunks WHERE bootstrap = ? AND component = ?",
        )
        .bind(id.as_slice())
        .bind(request.component.key())
        .fetch_one(&mut *tx)
        .await?;
        let mut outcome = if catalog.is_some() {
            PutOutcome::Quarantined
        } else {
            PutOutcome::Verified
        };
        if usize::try_from(stored)? == lengths.len() {
            let records = records(&mut tx, id, request.component).await?;
            if let Some(class) = catalog {
                let validation = d.catalog(class, &records.concat()).and_then(|catalog| {
                    let mut projected = layout.components.clone();
                    projected.extend(
                        d.artifacts(Some(&catalog))
                            .iter()
                            .map(|a| (a.component, a.lengths())),
                    );
                    Layout {
                        components: projected,
                        artifacts: Vec::new(),
                    }
                    .check_budget(c.budget)
                    .map_err(|_| crate::sync::bootstrap_format::Error::ResourceLimit)
                });
                if let Err(error) = validation {
                    advance(&mut tx, id, &c, Reclaim::Quarantine).await?;
                    sqlx::query("UPDATE server_bootstrap_candidates SET catalog_failure = ?, failure_reason = ? WHERE bootstrap = ?")
                        .bind(class as i64).bind(i64::from(error == crate::sync::bootstrap_format::Error::ResourceLimit)).bind(id.as_slice()).execute(&mut *tx).await?;
                    outcome = PutOutcome::CatalogRejected;
                } else {
                    sqlx::query("UPDATE server_bootstrap_chunks SET verified = 1 WHERE bootstrap = ? AND component = ?")
                        .bind(id.as_slice()).bind(request.component.key()).execute(&mut *tx).await?;
                    sqlx::query("UPDATE server_bootstrap_candidates SET catalog_failure = NULL, failure_reason = NULL WHERE bootstrap = ? AND catalog_failure = ?")
                        .bind(id.as_slice()).bind(class as i64).execute(&mut *tx).await?;
                    outcome = PutOutcome::Verified;
                }
            } else {
                layout
                    .artifacts
                    .iter()
                    .find(|a| a.component == request.component)
                    .unwrap()
                    .verify(&records)?;
            }
        }
        tx.commit().await?;
        Ok(outcome)
    }
}

async fn revalidate(conn: &mut SqliteConnection, id: &[u8; 32], layout: &Layout) -> Result<()> {
    for artifact in &layout.artifacts {
        let rows: Vec<(i64, Vec<u8>)> = sqlx::query_as(
            "SELECT chunk_index, bytes FROM server_bootstrap_chunks WHERE bootstrap = ? AND component = ? ORDER BY chunk_index",
        ).bind(id.as_slice()).bind(artifact.component.key()).fetch_all(&mut *conn).await?;
        for (index, bytes) in &rows {
            artifact.verify_chunk(usize::try_from(*index)?, bytes)?;
        }
        if rows.len() == artifact.lengths().len() {
            artifact.verify(&rows.into_iter().map(|(_, bytes)| bytes).collect::<Vec<_>>())?;
        }
    }
    Ok(())
}

async fn ensure_staging(
    conn: &mut SqliteConnection,
    id: &[u8; 32],
    commitment: [u8; 32],
) -> Result<StagingStatus> {
    let c = required(conn, id).await?;
    c.check(commitment, None)?;
    let retained = layout(conn, id, &c.declaration()?).await?;
    retained.check_budget(c.budget)?;
    revalidate(conn, id, &retained).await?;
    let now = now()?;
    if c.expires <= now {
        // Reclaimed candidates already have their next fence; expiry needs one.
        if c.expires != 0 {
            advance(conn, id, &c, Reclaim::Quarantine).await?;
        }
        let expires = now
            .checked_add(STAGING_TTL_SECONDS)
            .ok_or_else(|| anyhow::anyhow!("error bootstrap-clock"))?;
        sqlx::query("UPDATE server_bootstrap_candidates SET expires_at = ? WHERE bootstrap = ?")
            .bind(expires)
            .bind(id.as_slice())
            .execute(&mut *conn)
            .await?;
    }
    match status(conn, id).await? {
        Status::Staging(status) => Ok(status),
        _ => anyhow::bail!("error bootstrap-storage-invalid"),
    }
}
