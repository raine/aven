use super::super::{Context, hash, valid};
use super::{codec::Descriptor, *};
use crate::{
    attachments::lifecycle::LifecyclePolicy,
    db::{Database, begin_immediate},
    sync::seed_claim::Secret,
};
use anyhow::{Context as _, Result, ensure};
use sqlx::SqliteConnection;

pub(crate) async fn initialized(conn: &mut SqliteConnection, descriptor: &[u8; 32]) -> Result<()> {
    let found: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT descriptor FROM server_e2ee_image_initialization WHERE singleton=1",
    )
    .fetch_optional(conn)
    .await?;
    ensure!(
        found.as_deref() == Some(descriptor.as_slice()),
        "error encrypted-image-reinitialization-required"
    );
    Ok(())
}
async fn load(
    conn: &mut SqliteConnection,
    object: &[u8; 32],
    commitment: &[u8; 32],
) -> Result<(Descriptor, i64, bool)> {
    let (bytes, epoch, complete): (Option<Vec<u8>>, i64, bool) =
        sqlx::query_as("SELECT descriptor,epoch,complete FROM server_e2ee_images WHERE object=?")
            .bind(object.as_slice())
            .fetch_optional(conn)
            .await?
            .context("error encrypted-image-unknown")?;
    let bytes = bytes.context("error encrypted-image-reinitialization-required")?;
    valid(hash(&bytes) == *commitment)?;
    let d = Descriptor::decode(&bytes)?;
    valid(d.object == *object)?;
    Ok((d, epoch, complete))
}
pub(crate) async fn refresh(conn: &mut SqliteConnection, now: i64) -> Result<()> {
    sqlx::query("UPDATE server_e2ee_images SET unreferenced_at=CASE WHEN EXISTS(SELECT 1 FROM server_e2ee_image_references r JOIN server_e2ee_image_parents p ON p.workspace=r.workspace AND p.parent=r.parent WHERE r.object=server_e2ee_images.object AND r.deleted=0 AND (p.deleted=0 OR p.protected=1 OR p.version IS NULL)) OR EXISTS(SELECT 1 FROM server_e2ee_image_tickets t WHERE t.object=server_e2ee_images.object AND t.expires_at>?) THEN NULL ELSE COALESCE(unreferenced_at,?) END").bind(now).bind(now).execute(conn).await?;
    Ok(())
}
async fn protected(
    conn: &mut SqliteConnection,
    object: &[u8; 32],
    workspace: &str,
) -> Result<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM server_e2ee_image_references r JOIN server_e2ee_image_parents p ON p.workspace=r.workspace AND p.parent=r.parent WHERE r.object=? AND r.workspace=? AND r.deleted=0 AND (p.deleted=0 OR p.protected=1 OR p.version IS NULL))").bind(object.as_slice()).bind(workspace).fetch_one(conn).await?)
}
async fn scope(conn: &mut SqliteConnection, object: &[u8; 32], workspace: &str) -> Result<()> {
    valid(
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM server_e2ee_image_scopes WHERE object=? AND workspace=?)",
        )
        .bind(object.as_slice())
        .bind(workspace)
        .fetch_one(conn)
        .await?,
    )
}
async fn reserve(
    conn: &mut SqliteConnection,
    d: &Descriptor,
    workspace: &str,
    device: &[u8; 32],
    epoch: i64,
    quota: i64,
    now: i64,
) -> Result<()> {
    sqlx::query("DELETE FROM server_e2ee_image_tickets WHERE expires_at<=?")
        .bind(now)
        .execute(&mut *conn)
        .await?;
    let existing: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM server_e2ee_image_tickets WHERE object=? AND workspace=? AND device=? AND epoch=? AND expires_at>?)").bind(d.object.as_slice()).bind(workspace).bind(device.as_slice()).bind(epoch).bind(now).fetch_one(&mut *conn).await?;
    if existing {
        return Ok(());
    }
    let reactivating: bool = sqlx::query_scalar("SELECT origin IS NULL AND NOT EXISTS(SELECT 1 FROM server_e2ee_image_chunks c WHERE c.object=i.object) AND NOT EXISTS(SELECT 1 FROM server_e2ee_image_tickets t WHERE t.object=i.object AND t.expires_at>?) FROM server_e2ee_images i WHERE object=?")
        .bind(now).bind(d.object.as_slice()).fetch_one(&mut *conn).await?;
    if reactivating {
        let active: i64 = sqlx::query_scalar("SELECT count(*) FROM server_e2ee_images i WHERE origin IS NULL AND (EXISTS(SELECT 1 FROM server_e2ee_image_chunks c WHERE c.object=i.object) OR EXISTS(SELECT 1 FROM server_e2ee_image_tickets t WHERE t.object=i.object AND t.expires_at>?))")
            .bind(now).fetch_one(&mut *conn).await?;
        valid(active < 128)?;
    }
    let counted = protected(conn, &d.object, workspace).await? || sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM server_e2ee_image_tickets WHERE object=? AND workspace=? AND expires_at>?)").bind(d.object.as_slice()).bind(workspace).bind(now).fetch_one(&mut *conn).await?;
    if !counted {
        let used: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(i.byte_size),0) FROM server_e2ee_images i WHERE EXISTS(SELECT 1 FROM server_e2ee_image_references r JOIN server_e2ee_image_parents p ON p.workspace=r.workspace AND p.parent=r.parent WHERE r.object=i.object AND r.workspace=? AND r.deleted=0 AND (p.deleted=0 OR p.protected=1 OR p.version IS NULL)) OR EXISTS(SELECT 1 FROM server_e2ee_image_tickets t WHERE t.object=i.object AND t.workspace=? AND t.expires_at>?)").bind(workspace).bind(workspace).bind(now).fetch_one(&mut *conn).await?;
        ensure!(
            used.checked_add(d.byte_size()).is_some_and(|n| n <= quota),
            "error attachment-quota-exceeded"
        );
    }
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM server_e2ee_image_tickets WHERE expires_at>?")
            .bind(now)
            .fetch_one(&mut *conn)
            .await?;
    valid(count < 128)?;
    let mut id = [0; 32];
    getrandom::fill(&mut id).map_err(|_| anyhow::anyhow!("error encrypted-image-entropy"))?;
    let expires = now
        .checked_add(crate::attachments::lifecycle::LEASE_TTL.as_secs() as i64)
        .context("error encrypted-image-clock")?;
    sqlx::query("INSERT INTO server_e2ee_image_tickets(reservation,object,workspace,device,epoch,expires_at) VALUES(?,?,?,?,?,?) ON CONFLICT(object,workspace,device) DO UPDATE SET reservation=excluded.reservation,epoch=excluded.epoch,expires_at=excluded.expires_at")
        .bind(id.as_slice()).bind(d.object.as_slice()).bind(workspace).bind(device.as_slice()).bind(epoch).bind(expires).execute(&mut *conn).await?;
    sqlx::query("UPDATE server_e2ee_images SET unreferenced_at=NULL WHERE object=?")
        .bind(d.object.as_slice())
        .execute(conn)
        .await?;
    Ok(())
}
async fn ticket(
    conn: &mut SqliteConnection,
    object: &[u8; 32],
    workspace: &str,
    device: &[u8; 32],
    ticket: &Ticket,
    now: i64,
) -> Result<()> {
    valid(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM server_e2ee_image_tickets t JOIN server_e2ee_images i ON i.object=t.object WHERE t.object=? AND t.workspace=? AND t.device=? AND t.reservation=? AND t.epoch=? AND i.epoch=t.epoch AND t.expires_at>?)").bind(object.as_slice()).bind(workspace).bind(device.as_slice()).bind(ticket.reservation.as_slice()).bind(ticket.epoch).bind(now).fetch_one(conn).await?)
}
async fn status(
    conn: &mut SqliteConnection,
    d: &Descriptor,
    workspace: &str,
    device: &[u8; 32],
    now: i64,
) -> Result<Reply> {
    let (epoch, complete): (i64, bool) =
        sqlx::query_as("SELECT epoch,complete FROM server_e2ee_images WHERE object=?")
            .bind(d.object.as_slice())
            .fetch_one(&mut *conn)
            .await?;
    let indices: Vec<i64> = sqlx::query_scalar(
        "SELECT chunk_index FROM server_e2ee_image_chunks WHERE object=? ORDER BY chunk_index",
    )
    .bind(d.object.as_slice())
    .fetch_all(&mut *conn)
    .await?;
    let t:Option<(Vec<u8>,i64)>=sqlx::query_as("SELECT reservation,expires_at FROM server_e2ee_image_tickets WHERE object=? AND workspace=? AND device=? AND epoch=? AND expires_at>?").bind(d.object.as_slice()).bind(workspace).bind(device.as_slice()).bind(epoch).bind(now).fetch_optional(conn).await?;
    let (reservation, expires_at) = match t {
        Some((id, e)) => (
            Some(
                id.try_into()
                    .map_err(|_| anyhow::anyhow!("error encrypted-image-storage"))?,
            ),
            Some(e),
        ),
        None => (None, None),
    };
    Ok(Reply::Status(Status {
        epoch,
        complete,
        missing: (0..d.artifact.chunks.len())
            .filter(|i| !indices.contains(&(*i as i64)))
            .collect(),
        reservation,
        expires_at,
    }))
}
impl Database {
    pub async fn encrypted_image_exchange(
        &self,
        context: &Context,
        bearer: &Secret,
        op: Operation,
        policy: LifecyclePolicy,
    ) -> Result<Reply> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let current = crate::sync::seed_claim::membership::persistence::current(&mut tx).await?;
        current
            .membership
            .authenticate(&context.authentication(bearer), false)?;
        let binding = current.membership.publication().binding();
        valid(
            context.stream == binding.stream_id
                && context.descriptor == binding.descriptor_commitment,
        )?;
        initialized(&mut tx, &context.descriptor).await?;
        let now = chrono::Utc::now().timestamp();
        let reply = match op {
            Operation::Declare {
                workspace,
                descriptor,
            } => {
                workspace.parse::<crate::ids::WorkspaceId>()?;
                let d = Descriptor::decode(&descriptor)?;
                valid(
                    d.vault == context.vault
                        && d.stream == context.stream
                        && d.generation == current.membership.genesis().context().generation_id,
                )?;
                let old: Option<Option<Vec<u8>>> =
                    sqlx::query_scalar("SELECT descriptor FROM server_e2ee_images WHERE object=?")
                        .bind(d.object.as_slice())
                        .fetch_optional(&mut *tx)
                        .await?;
                if let Some(old) = old {
                    valid(old.as_deref() == Some(descriptor.as_slice()))?;
                } else {
                    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM server_e2ee_images")
                        .fetch_one(&mut *tx)
                        .await?;
                    valid(total < 65536)?;
                    sqlx::query("INSERT INTO server_e2ee_images(object,byte_size,descriptor,complete,unreferenced_at) VALUES(?,?,?,0,?)").bind(d.object.as_slice()).bind(d.byte_size()).bind(&descriptor).bind(now).execute(&mut *tx).await?;
                }
                let known:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM server_e2ee_image_scopes WHERE object=? AND workspace=?)").bind(d.object.as_slice()).bind(&workspace).fetch_one(&mut *tx).await?;
                if !known {
                    let count: i64 =
                        sqlx::query_scalar("SELECT count(*) FROM server_e2ee_image_scopes")
                            .fetch_one(&mut *tx)
                            .await?;
                    valid(count < 262144)?;
                }
                sqlx::query(
                    "INSERT OR IGNORE INTO server_e2ee_image_scopes(object,workspace) VALUES(?,?)",
                )
                .bind(d.object.as_slice())
                .bind(&workspace)
                .execute(&mut *tx)
                .await?;
                let (_, epoch, _) = load(&mut tx, &d.object, &hash(&descriptor)).await?;
                reserve(
                    &mut tx,
                    &d,
                    &workspace,
                    &context.device,
                    epoch,
                    policy.quota_bytes,
                    now,
                )
                .await?;
                status(&mut tx, &d, &workspace, &context.device, now).await?
            }
            Operation::Prune { limit } => {
                valid((1..=128).contains(&limit))?;
                refresh(&mut tx, now).await?;
                let cutoff = now
                    .checked_sub(i64::try_from(policy.grace.as_secs())?)
                    .context("error encrypted-image-clock")?;
                let objects:Vec<Vec<u8>>=sqlx::query_scalar("SELECT object FROM server_e2ee_images WHERE unreferenced_at<=? AND (complete=1 OR EXISTS(SELECT 1 FROM server_e2ee_image_chunks c WHERE c.object=server_e2ee_images.object)) ORDER BY object LIMIT ?").bind(cutoff).bind(limit as i64).fetch_all(&mut *tx).await?;
                for object in &objects {
                    valid(sqlx::query("UPDATE server_e2ee_images SET epoch=epoch+1,complete=0 WHERE object=? AND epoch<9223372036854775807").bind(object).execute(&mut *tx).await?.rows_affected()==1)?;
                    sqlx::query("DELETE FROM server_e2ee_image_chunks WHERE object=?")
                        .bind(object)
                        .execute(&mut *tx)
                        .await?;
                }
                Reply::Pruned(objects.len())
            }
            other => dispatch_existing(&mut tx, context, other, policy, now).await?,
        };
        tx.commit().await?;
        Ok(reply)
    }
}
async fn dispatch_existing(
    conn: &mut SqliteConnection,
    context: &Context,
    op: Operation,
    policy: LifecyclePolicy,
    now: i64,
) -> Result<Reply> {
    let (workspace, object, commitment) = match &op {
        Operation::Status {
            workspace,
            object,
            descriptor_commitment,
        }
        | Operation::Ensure {
            workspace,
            object,
            descriptor_commitment,
            ..
        }
        | Operation::Put {
            workspace,
            object,
            descriptor_commitment,
            ..
        }
        | Operation::Complete {
            workspace,
            object,
            descriptor_commitment,
            ..
        }
        | Operation::Read {
            workspace,
            object,
            descriptor_commitment,
            ..
        }
        | Operation::Release {
            workspace,
            object,
            descriptor_commitment,
            ..
        } => (workspace, *object, *descriptor_commitment),
        _ => unreachable!(),
    };
    workspace.parse::<crate::ids::WorkspaceId>()?;
    scope(conn, &object, workspace).await?;
    let (d, epoch, _) = load(conn, &object, &commitment).await?;
    valid(d.vault == context.vault && d.stream == context.stream)?;
    match &op {
        Operation::Status { .. } => status(conn, &d, workspace, &context.device, now).await,
        Operation::Ensure { expected_epoch, .. } => {
            valid(*expected_epoch == epoch)?;
            // An epoch is shared by all holders. Never fence another live promise.
            let live:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM server_e2ee_image_tickets WHERE object=? AND expires_at>?)").bind(object.as_slice()).bind(now).fetch_one(&mut *conn).await?;
            let next = if live {
                epoch
            } else {
                epoch
                    .checked_add(1)
                    .context("error encrypted-image-epoch")?
            };
            sqlx::query("UPDATE server_e2ee_images SET epoch=? WHERE object=?")
                .bind(next)
                .bind(object.as_slice())
                .execute(&mut *conn)
                .await?;
            reserve(
                conn,
                &d,
                workspace,
                &context.device,
                next,
                policy.quota_bytes,
                now,
            )
            .await?;
            status(conn, &d, workspace, &context.device, now).await
        }
        Operation::Read { index, .. } => {
            valid(*index < d.artifact.chunks.len())?;
            let admitted: bool = sqlx::query_scalar(
                "SELECT origin IS NOT NULL FROM server_e2ee_images WHERE object=?",
            )
            .bind(object.as_slice())
            .fetch_one(&mut *conn)
            .await?;
            valid(admitted)?;
            let bytes: Option<Vec<u8>> = sqlx::query_scalar(
                "SELECT bytes FROM server_e2ee_image_chunks WHERE object=? AND chunk_index=?",
            )
            .bind(object.as_slice())
            .bind(*index as i64)
            .fetch_optional(conn)
            .await?;
            Ok(match bytes {
                Some(b) => {
                    d.verify_chunk(*index, &b)?;
                    Reply::Chunk(b)
                }
                None => Reply::Unavailable,
            })
        }
        Operation::Put {
            epoch: e,
            reservation,
            ..
        }
        | Operation::Complete {
            epoch: e,
            reservation,
            ..
        }
        | Operation::Release {
            epoch: e,
            reservation,
            ..
        } => {
            ticket(
                conn,
                &object,
                workspace,
                &context.device,
                &Ticket {
                    epoch: *e,
                    reservation: *reservation,
                },
                now,
            )
            .await?;
            match &op {
                Operation::Put { index, record, .. } => {
                    d.verify_chunk(*index, record)?;
                    let old:Option<Vec<u8>>=sqlx::query_scalar("SELECT bytes FROM server_e2ee_image_chunks WHERE object=? AND chunk_index=?").bind(object.as_slice()).bind(*index as i64).fetch_optional(&mut *conn).await?;
                    valid(old.as_ref().is_none_or(|b| b == record))?;
                    sqlx::query("INSERT OR IGNORE INTO server_e2ee_image_chunks(object,chunk_index,bytes) VALUES(?,?,?)").bind(object.as_slice()).bind(*index as i64).bind(record).execute(&mut *conn).await?;
                }
                Operation::Complete { .. } => {
                    let records:Vec<Vec<u8>>=sqlx::query_scalar("SELECT bytes FROM server_e2ee_image_chunks WHERE object=? ORDER BY chunk_index").bind(object.as_slice()).fetch_all(&mut *conn).await?;
                    d.verify(&records)?;
                    sqlx::query("UPDATE server_e2ee_images SET complete=1 WHERE object=?")
                        .bind(object.as_slice())
                        .execute(&mut *conn)
                        .await?;
                }
                Operation::Release { .. } => {
                    sqlx::query("DELETE FROM server_e2ee_image_tickets WHERE reservation=?")
                        .bind(reservation.as_slice())
                        .execute(&mut *conn)
                        .await?;
                    refresh(conn, now).await?;
                }
                _ => unreachable!(),
            }
            Ok(Reply::Done)
        }
        _ => unreachable!(),
    }
}

pub(in crate::sync::encrypted_tail) async fn admit(
    conn: &mut SqliteConnection,
    context: &Context,
    id: &str,
    p: &super::super::domain::Projection,
    t: Option<&Ticket>,
) -> Result<()> {
    use super::super::domain::Projection;
    let now = chrono::Utc::now().timestamp();
    match p {
        Projection::Ref {
            workspace,
            task,
            reference,
            descriptor,
            deleted,
            version,
        } => {
            initialized(conn, &context.descriptor).await?;
            let d = Descriptor::decode(descriptor)?;
            let (_, _, complete) = load(conn, &d.object, &hash(descriptor)).await?;
            valid(complete && d.vault == context.vault && d.stream == context.stream)?;
            let records: Vec<Vec<u8>> = sqlx::query_scalar(
                "SELECT bytes FROM server_e2ee_image_chunks WHERE object=? ORDER BY chunk_index",
            )
            .bind(d.object.as_slice())
            .fetch_all(&mut *conn)
            .await?;
            d.verify(&records)?;
            let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM server_e2ee_image_references WHERE workspace=? AND reference=?)").bind(workspace).bind(reference).fetch_one(&mut *conn).await?;
            valid(!exists)?;
            if !protected(conn, &d.object, workspace).await? {
                ticket(
                    conn,
                    &d.object,
                    workspace,
                    &context.device,
                    t.context("error encrypted-image-reservation-required")?,
                    now,
                )
                .await?;
            }
            let count: i64 =
                sqlx::query_scalar("SELECT count(*) FROM server_e2ee_image_references")
                    .fetch_one(&mut *conn)
                    .await?;
            valid(count < 262144)?;
            let state:Option<(Option<String>,bool,bool)>=sqlx::query_as("SELECT version,deleted,protected FROM server_e2ee_image_parents WHERE workspace=? AND parent=?").bind(workspace).bind(task).fetch_optional(&mut *conn).await?;
            if state.is_none() {
                let count: i64 =
                    sqlx::query_scalar("SELECT count(*) FROM server_e2ee_image_parents")
                        .fetch_one(&mut *conn)
                        .await?;
                valid(count < 262144)?;
            }
            let (known, state_deleted, sticky) = state.unwrap_or((None, false, false));
            let mut state = crate::sync::persistence::parent_liveness::ParentState {
                version: known.clone(),
                deleted: state_deleted,
                protected: sticky,
            };
            state.protect_hint(*deleted, version.as_deref());
            sqlx::query("INSERT INTO server_e2ee_image_parents(workspace,parent,version,deleted,protected) VALUES(?,?,?,?,?) ON CONFLICT(workspace,parent) DO UPDATE SET protected=excluded.protected").bind(workspace).bind(task).bind(known).bind(state_deleted).bind(state.protected).execute(&mut *conn).await?;
            sqlx::query("INSERT INTO server_e2ee_image_references(workspace,reference,parent,deleted,object) VALUES(?,?,?,0,?)").bind(workspace).bind(reference).bind(task).bind(d.object.as_slice()).execute(&mut *conn).await?;
            sqlx::query("UPDATE server_e2ee_images SET origin=COALESCE(origin,?) WHERE object=?")
                .bind(id)
                .bind(d.object.as_slice())
                .execute(&mut *conn)
                .await?;
            if let Some(t) = t {
                sqlx::query("DELETE FROM server_e2ee_image_tickets WHERE reservation=? AND device=? AND workspace=? AND object=? AND epoch=?").bind(t.reservation.as_slice()).bind(context.device.as_slice()).bind(workspace).bind(d.object.as_slice()).bind(t.epoch).execute(&mut *conn).await?;
            }
            refresh(conn, now).await?;
        }
        Projection::Unref {
            workspace,
            task,
            reference,
        } => {
            initialized(conn, &context.descriptor).await?;
            let changed=sqlx::query("UPDATE server_e2ee_image_references SET deleted=1 WHERE workspace=? AND reference=? AND parent=?").bind(workspace).bind(reference).bind(task).execute(&mut *conn).await?.rows_affected();
            valid(changed == 1)?;
            refresh(conn, now).await?;
        }
        _ => valid(t.is_none())?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn retired_declaration_cannot_bypass_active_object_limit() {
        let root = tempfile::tempdir().unwrap();
        let db = Database::open(&root.path().join("test.sqlite"))
            .await
            .unwrap();
        let a = crate::sync::encrypted_tail::tests::authority();
        let mut conn = db.acquire_writer().await.unwrap();
        let mut tx = begin_immediate(&mut conn).await.unwrap();
        let (retired, _) = Descriptor::seal(&a, b"x").unwrap();
        sqlx::query("INSERT INTO server_e2ee_images(object,byte_size,descriptor) VALUES(?,?,?)")
            .bind(retired.object.as_slice())
            .bind(retired.byte_size())
            .bind(retired.encode().unwrap())
            .execute(&mut *tx)
            .await
            .unwrap();
        // These are unadmitted staging objects, not fabricated accepted references.
        for _ in 0..128 {
            let (d, records) = Descriptor::seal(&a, b"x").unwrap();
            sqlx::query(
                "INSERT INTO server_e2ee_images(object,byte_size,descriptor) VALUES(?,?,?)",
            )
            .bind(d.object.as_slice())
            .bind(d.byte_size())
            .bind(d.encode().unwrap())
            .execute(&mut *tx)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO server_e2ee_image_chunks(object,chunk_index,bytes) VALUES(?,0,?)",
            )
            .bind(d.object.as_slice())
            .bind(&records[0])
            .execute(&mut *tx)
            .await
            .unwrap();
        }
        assert!(
            reserve(
                &mut tx,
                &retired,
                "0000000000000000",
                &a.context.device,
                1,
                i64::MAX,
                123
            )
            .await
            .is_err()
        );
        let tickets: i64 = sqlx::query_scalar("SELECT count(*) FROM server_e2ee_image_tickets")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(tickets, 0);
    }
}
