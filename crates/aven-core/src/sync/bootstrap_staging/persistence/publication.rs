use anyhow::Context as _;

use super::*;
use crate::sync::seed_claim::{PUBLICATION_BYTES, Publication, PublicationOutcome};

/// Only verified supported membership chains can authorize requests.
/// A historical outcome is never itself the current membership checkpoint.
pub(super) async fn authorize_current(
    conn: &mut SqliteConnection,
    auth: &Authentication<'_>,
) -> Result<(Genesis, Option<PublicationOutcome>)> {
    let row: Option<(Vec<u8>, bool)> =
        sqlx::query_as("SELECT genesis, genesis_only FROM server_seed_claim WHERE singleton = 1")
            .fetch_optional(&mut *conn)
            .await?;
    let (record, genesis_only) =
        row.ok_or_else(|| anyhow::anyhow!("error bootstrap-unauthorized"))?;
    let genesis = Genesis::from_record(&record)?;
    ensure!(
        genesis.authorizes_bearer(auth.bearer)
            && genesis.context().vault_id == auth.vault_id
            && genesis.commitment() == auth.genesis_commitment,
        "error bootstrap-unauthorized"
    );
    let head: Option<(i64, Vec<u8>)> = sqlx::query_as(
        "SELECT sequence, commitment FROM server_e2ee_membership_head WHERE singleton = 1",
    )
    .fetch_optional(&mut *conn)
    .await?;
    let saved: Option<(Vec<u8>, Vec<u8>, Vec<u8>)> = sqlx::query_as(
        "SELECT bootstrap, descriptor, signed_record FROM server_bootstrap_publication WHERE singleton = 1",
    ).fetch_optional(&mut *conn).await?;
    if genesis_only {
        ensure!(
            head.is_none() && saved.is_none(),
            "error bootstrap-membership-unsupported"
        );
        return Ok((genesis, None));
    }
    let current = crate::sync::seed_claim::membership::persistence::current(conn).await?;
    current.membership.authenticate(
        &crate::sync::seed_claim::peer::Authentication {
            vault: auth.vault_id,
            genesis: auth.genesis_commitment,
            device: genesis.device_id(),
            credential_version: 1,
            head: current.membership.head(),
            bearer: auth.bearer,
        },
        false,
    )?;
    let publication = current.membership.publication().clone();
    let (id, _, _) = saved.context("error bootstrap-storage-invalid")?;
    ensure!(
        id == publication.binding().bootstrap_id,
        "error bootstrap-storage-invalid"
    );
    Ok((
        current.membership.genesis().clone(),
        Some(PublicationOutcome { publication }),
    ))
}

async fn complete(
    conn: &mut SqliteConnection,
    id: &[u8; 32],
    d: &DeclarationView,
    budget: Budget,
) -> Result<()> {
    // Missing describing catalogs must not reduce the required artifact set.
    for (class, component) in CATALOGS.into_iter().enumerate() {
        let lengths = d.catalog_lengths(class)?;
        let rows = complete_records(conn, id, component, &lengths).await?;
        d.catalog(class, &rows.concat())?;
    }
    let layout = layout(conn, id, d).await?;
    layout.check_budget(budget)?;
    let expected: usize = layout
        .components
        .iter()
        .map(|(_, lengths)| lengths.len())
        .sum();
    let actual: i64 =
        sqlx::query_scalar("SELECT count(*) FROM server_bootstrap_chunks WHERE bootstrap = ?")
            .bind(id.as_slice())
            .fetch_one(&mut *conn)
            .await?;
    ensure!(
        usize::try_from(actual)? == expected,
        "error bootstrap-incomplete"
    );
    for artifact in &layout.artifacts {
        let records = complete_records(conn, id, artifact.component, &artifact.lengths()).await?;
        artifact.verify(&records)?;
    }
    Ok(())
}

async fn complete_records(
    conn: &mut SqliteConnection,
    id: &[u8; 32],
    component: Component,
    lengths: &[u64],
) -> Result<Vec<Vec<u8>>> {
    let rows: Vec<(i64, bool, Vec<u8>)> = sqlx::query_as(
        "SELECT chunk_index, verified, bytes FROM server_bootstrap_chunks WHERE bootstrap = ? AND component = ? ORDER BY chunk_index",
    ).bind(id.as_slice()).bind(component.key()).fetch_all(&mut *conn).await?;
    ensure!(rows.len() == lengths.len(), "error bootstrap-incomplete");
    let mut result = Vec::with_capacity(rows.len());
    for (expected, (index, verified, bytes)) in rows.into_iter().enumerate() {
        ensure!(
            usize::try_from(index)? == expected
                && verified
                && bytes.len() as u64 == lengths[expected],
            "error bootstrap-incomplete"
        );
        result.push(bytes);
    }
    Ok(result)
}

impl Database {
    /// Atomically publishes one complete immutable candidate and signed successor.
    /// This does not adopt the seed, install a client, or enable encrypted tail sync.
    pub async fn publish_bootstrap(
        &self,
        auth: &Authentication<'_>,
        request: PublishBootstrap<'_>,
        policy: PublicationPolicy,
    ) -> Result<PublicationOutcome> {
        ensure!(
            request.record.len() == PUBLICATION_BYTES,
            "error bootstrap-publication-invalid"
        );
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let (genesis, published) = authorize_current(&mut tx, auth).await?;
        if let Some(outcome) = published {
            let publication = outcome.publication();
            ensure!(
                publication.record() == request.record
                    && publication.binding().bootstrap_id == request.bootstrap_id
                    && publication.binding().descriptor_commitment == request.descriptor_commitment,
                "error bootstrap-publication-conflict"
            );
            crate::sync::encrypted_tail::attachments::server::initialized(
                &mut tx,
                &request.descriptor_commitment,
            )
            .await?;
            tx.commit().await?;
            return Ok(outcome);
        }
        let id = &request.bootstrap_id;
        let c = required(&mut tx, id).await?;
        c.check(request.descriptor_commitment, Some(request.epoch))?;
        let published_at = now()?;
        ensure!(c.expires > published_at, "error bootstrap-staging-expired");
        let descriptor = c
            .descriptor
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("error bootstrap-storage-invalid"))?;
        let publication = Publication::from_record(&genesis, descriptor, request.record)?;
        ensure!(
            publication.binding().bootstrap_id == *id,
            "error bootstrap-publication-conflict"
        );
        let d = c.declaration()?;
        complete(&mut tx, id, &d, c.budget).await?;
        let prefix = d.prefix_rows(
            &records(&mut tx, id, Component::PrefixCatalog)
                .await?
                .concat(),
        )?;
        let images = d.image_rows(
            &records(&mut tx, id, Component::ImageCatalog)
                .await?
                .concat(),
        )?;
        ensure!(
            images.parents.len() <= 262144 && images.references.len() <= 262144,
            "error encrypted-image-metadata-limit"
        );
        sqlx::query("INSERT INTO server_bootstrap_publication(singleton, bootstrap, descriptor, signed_record, published_at) VALUES (1, ?, ?, ?, ?)")
            .bind(id.as_slice()).bind(descriptor).bind(request.record).bind(published_at).execute(&mut *tx).await?;
        for (rank, operation) in prefix {
            sqlx::query("INSERT INTO server_bootstrap_prefix(operation_id, rank) VALUES (?, ?)")
                .bind(operation)
                .bind(i64::try_from(rank)?)
                .execute(&mut *tx)
                .await?;
        }
        for parent in images.parents {
            sqlx::query("INSERT INTO server_e2ee_image_parents(workspace, parent, deleted, protected, version) VALUES (?, ?, ?, ?, ?)")
                .bind(parent.workspace).bind(parent.task).bind(parent.deleted).bind(parent.protected).bind(parent.version)
                .execute(&mut *tx).await?;
        }
        sqlx::query(
            "INSERT INTO server_e2ee_image_initialization(singleton,descriptor) VALUES(1,?)",
        )
        .bind(request.descriptor_commitment.as_slice())
        .execute(&mut *tx)
        .await?;
        for image in images.objects {
            let binding = d.binding();
            let image_descriptor = crate::sync::encrypted_tail::attachments::codec::Descriptor {
                vault: binding.vault,
                stream: binding.stream,
                generation: binding.generation,
                object: image.id,
                artifact: image.artifact.clone(),
            }
            .encode()?;
            let bytes: u64 = image.artifact.chunks.iter().map(|c| c.length).sum();
            sqlx::query("INSERT INTO server_e2ee_images(object, bootstrap, byte_size, unreferenced_at,descriptor,origin,complete) VALUES (?, ?, ?, ?,?,'bootstrap',1)")
                .bind(image.id.as_slice()).bind(id.as_slice()).bind(i64::try_from(bytes)?).bind(published_at).bind(image_descriptor).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO server_e2ee_image_chunks(object, chunk_index, bytes) SELECT ?, chunk_index, bytes FROM server_bootstrap_chunks WHERE bootstrap = ? AND component = ?")
                .bind(image.id.as_slice()).bind(id.as_slice()).bind(Component::Image(image.id).key()).execute(&mut *tx).await?;
            sqlx::query(
                "DELETE FROM server_bootstrap_chunks WHERE bootstrap = ? AND component = ?",
            )
            .bind(id.as_slice())
            .bind(Component::Image(image.id).key())
            .execute(&mut *tx)
            .await?;
        }
        for reference in images.references {
            if let Some(object) = reference.object {
                sqlx::query(
                    "INSERT OR IGNORE INTO server_e2ee_image_scopes(object,workspace) VALUES(?,?)",
                )
                .bind(object.as_slice())
                .bind(&reference.workspace)
                .execute(&mut *tx)
                .await?;
            }
            sqlx::query("INSERT INTO server_e2ee_image_references(workspace, reference, parent, deleted, object) VALUES (?, ?, ?, ?, ?)")
                .bind(reference.workspace).bind(reference.reference).bind(reference.task).bind(reference.deleted)
                .bind(reference.object.map(|id| id.to_vec())).execute(&mut *tx).await?;
        }
        sqlx::query(
            "UPDATE server_e2ee_images SET unreferenced_at = NULL WHERE object IN (
            SELECT r.object FROM server_e2ee_image_references r JOIN server_e2ee_image_parents p
            ON p.workspace = r.workspace AND p.parent = r.parent
            WHERE r.deleted = 0 AND (p.deleted = 0 OR p.protected = 1 OR p.version IS NULL))",
        )
        .execute(&mut *tx)
        .await?;
        // Charge distinct protected objects per workspace, not historical roots or
        // grace bytes. The staging reservation already bounds all retained bytes.
        let usage: Vec<i64> = sqlx::query_scalar("SELECT sum(byte_size) FROM (
            SELECT DISTINCT r.workspace, i.object, i.byte_size FROM server_e2ee_image_references r
            JOIN server_e2ee_image_parents p ON p.workspace = r.workspace AND p.parent = r.parent
            JOIN server_e2ee_images i ON i.object = r.object
            WHERE r.deleted = 0 AND (p.deleted = 0 OR p.protected = 1 OR p.version IS NULL)) GROUP BY workspace")
            .fetch_all(&mut *tx).await?;
        ensure!(
            usage
                .into_iter()
                .all(|bytes| bytes >= 0 && bytes as u64 <= policy.workspace_quota_bytes),
            "error attachment-quota-exceeded"
        );
        let binding = publication.binding();
        sqlx::query("INSERT INTO server_e2ee_allocator(singleton, stream, prefix_count, high_water) VALUES (1, ?, ?, ?)")
            .bind(binding.stream_id.as_slice()).bind(i64::try_from(binding.prefix_count)?).bind(i64::try_from(binding.prefix_count)?)
            .execute(&mut *tx).await?;
        sqlx::query("INSERT INTO server_e2ee_membership_head(singleton, sequence, commitment) VALUES (1, 1, ?)")
            .bind(publication.commitment().as_slice()).execute(&mut *tx).await?;
        sqlx::query("UPDATE server_seed_claim SET genesis_only = 0 WHERE singleton = 1")
            .execute(&mut *tx)
            .await?;
        // Publication consumes the active staging reservation; immutable budgets
        // remain declaration identity, not a renewable image pin.
        sqlx::query("UPDATE server_bootstrap_candidates SET expires_at = 0 WHERE bootstrap = ?")
            .bind(id.as_slice())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(PublicationOutcome { publication })
    }
}
