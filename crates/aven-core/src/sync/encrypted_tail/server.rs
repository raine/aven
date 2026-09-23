use super::*;
use crate::db::{Database, begin_immediate};
use crate::sync::persistence::parent_liveness::ParentState;
use anyhow::Context as _;
use sqlx::SqliteConnection;

async fn found(conn: &mut SqliteConnection, id: &str) -> Result<Option<Accepted>> {
    let row: Option<(i64, Vec<u8>, Vec<u8>)> = sqlx::query_as(
        "SELECT sequence,commitment,record FROM server_e2ee_tail WHERE operation_id=?",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?;
    row.map(|(sequence, commitment, record)| {
        Ok(Accepted {
            mapping: Mapping {
                operation_id: id.into(),
                sequence,
                commitment: commitment
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("error encrypted-tail-storage"))?,
            },
            record,
        })
    })
    .transpose()
}
async fn prefix(conn: &mut SqliteConnection, id: &str) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM server_bootstrap_prefix WHERE operation_id=?)",
    )
    .bind(id)
    .fetch_one(conn)
    .await?)
}
impl Database {
    pub async fn encrypted_tail_exchange(
        &self,
        context: &Context,
        bearer: &super::super::seed_claim::Secret,
        op: Operation,
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
        let n = i64::try_from(binding.prefix_count)?;
        let high = i64::try_from(
            crate::sync::seed_claim::membership::persistence::allocator(
                &mut tx,
                &current.membership,
            )
            .await?,
        )?;
        let reply = match op {
            Operation::Append { record, ticket } => {
                let e = codec::parse(&record)?;
                valid(e.vault == context.vault && e.stream == context.stream)?;
                ensure!(
                    !prefix(&mut tx, &e.id).await?,
                    "error encrypted-tail-prefix-identity-collision"
                );
                if let Some(old) = found(&mut tx, &e.id).await? {
                    Reply::Appended(old.mapping)
                } else {
                    ensure!(
                        !current.membership.rotation_pending(),
                        "error membership-rotation-pending"
                    );
                    valid(e.generation == current.membership.current_generation().id)?;
                    let sequence = high
                        .checked_add(1)
                        .context("error encrypted-tail-sequence-exhausted")?;
                    let mapping = Mapping {
                        operation_id: e.id.clone(),
                        sequence,
                        commitment: hash(&record),
                    };
                    sqlx::query("INSERT INTO server_e2ee_tail(operation_id,sequence,commitment,record) VALUES(?,?,?,?)")
                        .bind(&e.id).bind(sequence).bind(mapping.commitment.as_slice()).bind(&record).execute(&mut *tx).await?;
                    super::attachments::server::admit(
                        &mut tx,
                        context,
                        &current.membership,
                        &e.id,
                        &e.projection,
                        ticket.as_ref(),
                    )
                    .await?;
                    apply_parent(&mut tx, &e.id, &e.projection).await?;
                    sqlx::query("UPDATE server_e2ee_allocator SET high_water=? WHERE singleton=1")
                        .bind(sequence)
                        .execute(&mut *tx)
                        .await?;
                    Reply::Appended(mapping)
                }
            }
            Operation::Lookup {
                operation_id,
                expected,
            } => {
                valid(!operation_id.is_empty() && operation_id.len() <= 256)?;
                if let Some(want) = &expected {
                    valid(want.operation_id == operation_id)?;
                }
                if prefix(&mut tx, &operation_id).await? {
                    valid(expected.is_none())?;
                    Reply::Bootstrap
                } else if let Some(record) = found(&mut tx, &operation_id).await? {
                    valid(expected.as_ref().is_none_or(|m| m == &record.mapping))?;
                    Reply::Found(record)
                } else {
                    valid(expected.is_none())?;
                    Reply::Absent
                }
            }
            Operation::Pull {
                after,
                limit,
                watermark,
            } => {
                let watermark = watermark.unwrap_or(high);
                valid(
                    after >= n
                        && after <= watermark
                        && watermark <= high
                        && (1..=PAGE_COUNT).contains(&limit),
                )?;
                let rows:Vec<(String,i64,Vec<u8>,Vec<u8>)>=sqlx::query_as("SELECT operation_id,sequence,commitment,record FROM server_e2ee_tail WHERE sequence>? AND sequence<=? ORDER BY sequence LIMIT ?")
                    .bind(after).bind(watermark).bind(limit as i64).fetch_all(&mut *tx).await?;
                let mut records = Vec::new();
                let mut size = 0;
                let mut cursor = after;
                for (id, sequence, commitment, record) in rows {
                    if size + record.len() > PAGE_BYTES {
                        break;
                    }
                    size += record.len();
                    cursor = sequence;
                    records.push(Accepted {
                        mapping: Mapping {
                            operation_id: id,
                            sequence,
                            commitment: commitment
                                .try_into()
                                .map_err(|_| anyhow::anyhow!("error encrypted-tail-storage"))?,
                        },
                        record,
                    });
                }
                let has_more:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM server_e2ee_tail WHERE sequence>? AND sequence<=?)").bind(cursor).bind(watermark).fetch_one(&mut *tx).await?;
                valid(!has_more || cursor > after)?;
                Reply::Page(Page {
                    after,
                    watermark,
                    cursor,
                    has_more,
                    records,
                })
            }
        };
        tx.commit().await?;
        Ok(reply)
    }
}
async fn apply_parent(conn: &mut SqliteConnection, id: &str, p: &domain::Projection) -> Result<()> {
    let domain::Projection::Parent {
        action,
        workspace,
        task,
        deleted,
        version,
    } = p
    else {
        return Ok(());
    };
    let existing:Option<(Option<String>,bool,bool)>=sqlx::query_as("SELECT version,deleted,protected FROM server_e2ee_image_parents WHERE workspace=? AND parent=?").bind(workspace).bind(task).fetch_optional(&mut *conn).await?;
    if existing.is_none() {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM server_e2ee_image_parents")
            .fetch_one(&mut *conn)
            .await?;
        valid(count < 262144)?;
    }
    let mut state = existing
        .map(|(version, deleted, protected)| ParentState {
            version,
            deleted,
            protected,
        })
        .unwrap_or_default();
    state.apply(*action, id, *deleted, version.as_deref());
    sqlx::query("INSERT INTO server_e2ee_image_parents(workspace,parent,version,deleted,protected) VALUES(?,?,?,?,?) ON CONFLICT(workspace,parent) DO UPDATE SET version=excluded.version,deleted=excluded.deleted,protected=excluded.protected")
        .bind(workspace).bind(task).bind(state.version).bind(state.deleted).bind(state.protected).execute(&mut *conn).await?;
    super::attachments::server::refresh(conn, chrono::Utc::now().timestamp()).await?;
    Ok(())
}

#[cfg(test)]
mod rotation_tests;
