//! In-process encrypted sync between replicas and one server database.
//!
//! Records travel through the same database calls as the HTTP transport:
//! clients freeze and seal heads, the server appends and pages ciphertext,
//! and clients apply pages through the encrypted tail. Every replica shares
//! the seed device's authority, so replica identity comes only from each
//! database's own client ID.
use anyhow::{Result, bail};

use crate::data_safety::TaskDependencyRow;
use crate::db::{self, Database, begin_immediate};
use crate::sync::encrypted_tail::{
    self as tail, Accepted, Authority, Context, Operation, Reply, attachments, dependencies,
};
use crate::sync::seed_claim::membership::{Membership, test_support::Fixture};
use crate::sync::wire::ChangeWire;

const ASSOCIATION: &str = "encrypted-sync-test";
const GENERATION: i64 = 1;

pub struct EncryptedSyncServer {
    fixture: Fixture,
    membership: Membership,
}

impl EncryptedSyncServer {
    /// A server holding a published bootstrap and no tail records.
    pub async fn new() -> Self {
        Self::from_fixture(Fixture::new().await)
    }

    fn from_fixture(fixture: Fixture) -> Self {
        let membership = Membership::from_publication(
            fixture.seed.genesis(),
            &fixture.package.descriptor,
            fixture.publication.record(),
        )
        .unwrap();
        Self {
            fixture,
            membership,
        }
    }

    /// A server whose bootstrap prefix is `source`'s current shared state.
    /// Callers mark the prefix history of replicas holding that state as
    /// acknowledged before binding them.
    pub async fn publishing(source: &Database, blob_dir: &std::path::Path) -> Self {
        let dir = tempfile::tempdir().unwrap();
        Self::from_fixture(Fixture::publishing(dir, source, blob_dir).await)
    }

    /// Changes in the published prefix.
    pub fn prefix(&self) -> i64 {
        self.membership.publication().binding().prefix_count as i64
    }

    /// The server database, for direct inspection.
    pub fn database(&self) -> &Database {
        &self.fixture.db
    }

    fn authority(&self) -> Authority {
        let m = &self.membership;
        let binding = m.publication().binding();
        Authority {
            context: Context {
                vault: m.genesis().context().vault_id,
                genesis: m.genesis().commitment(),
                device: self.fixture.seed.genesis().device_id(),
                head: m.head(),
                stream: binding.stream_id,
                descriptor: binding.descriptor_commitment,
            },
            membership: m.clone(),
            keys: m.verify_initial_key(&self.fixture.key).unwrap(),
            prefix: binding.prefix_count as i64,
            association: ASSOCIATION.into(),
            sync_generation: GENERATION,
        }
    }

    async fn exchange(&self, a: &Authority, operation: Operation) -> Result<Reply> {
        self.fixture
            .db
            .encrypted_tail_exchange(&a.context, self.fixture.seed.bearer(), operation)
            .await
    }

    /// Associates `db` with this server at the published prefix. Existing
    /// local changes stay pending and push like later writes. Idempotent.
    pub async fn bind(&self, db: &Database) -> Result<()> {
        let prefix = self.authority().prefix;
        let mut conn = db.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        if db::get_meta(&mut tx, "e2ee_association").await?.is_some() {
            return Ok(());
        }
        db::set_meta(&mut tx, "e2ee_association", ASSOCIATION).await?;
        db::set_meta(&mut tx, "sync_generation", &GENERATION.to_string()).await?;
        db::set_meta(&mut tx, "sync_cursor", &prefix.to_string()).await?;
        // Pending dependency commands replay idempotently over the current graph.
        let edges: Vec<(String, String, String, String)> = sqlx::query_as(
            "SELECT workspace_id, task_id, depends_on_task_id, created_at FROM task_dependencies",
        )
        .fetch_all(&mut *tx)
        .await?;
        let edges = edges
            .into_iter()
            .map(|(workspace_id, task_id, depends_on_task_id, created_at)| {
                Ok(TaskDependencyRow {
                    workspace_id: workspace_id.parse()?,
                    task_id: task_id.parse()?,
                    depends_on_task_id: depends_on_task_id.parse()?,
                    created_at,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        dependencies::initialize(&mut tx, ASSOCIATION, GENERATION, prefix, &edges).await?;
        attachments::client::initialize(
            &mut tx,
            ASSOCIATION,
            GENERATION,
            prefix,
            &self.fixture.package,
            &self.fixture.key,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Appends the next pending change. Returns false when none is pending.
    pub async fn push_one(&self, db: &Database) -> Result<bool> {
        self.bind(db).await?;
        let a = self.authority();
        let Some(tail::Push { record, upload }) = db
            .prepare_encrypted_push(&a, self.fixture.dir.path())
            .await?
        else {
            return Ok(false);
        };
        if upload.is_some() {
            bail!("encrypted sync test server does not store images");
        }
        let Reply::Appended(mapping) = self
            .exchange(
                &a,
                Operation::Append {
                    ticket: None,
                    record,
                },
            )
            .await?
        else {
            bail!("unexpected append reply");
        };
        let frozen = db.observe_encrypted_tail(&a, &mapping).await?;
        // An identical operation ID may already hold another replica's record.
        let accepted = if tail::hash(&frozen) == mapping.commitment {
            Accepted {
                mapping,
                record: frozen,
            }
        } else {
            let Reply::Found(accepted) = self
                .exchange(
                    &a,
                    Operation::Lookup {
                        operation_id: mapping.operation_id.clone(),
                        expected: Some(mapping),
                    },
                )
                .await?
            else {
                bail!("unexpected lookup reply");
            };
            accepted
        };
        db.verify_encrypted_tail_outcome(&a, &accepted).await?;
        Ok(true)
    }

    /// Fetches the page after `db`'s cursor without applying it.
    pub async fn fetch(&self, db: &Database, limit: usize) -> Result<tail::Page> {
        self.bind(db).await?;
        let a = self.authority();
        let state = db.encrypted_round_state(&a).await?;
        let Reply::Page(page) = self
            .exchange(
                &a,
                Operation::Pull {
                    after: state.cursor,
                    limit,
                    watermark: state.initial_watermark,
                },
            )
            .await?
        else {
            bail!("unexpected pull reply");
        };
        Ok(page)
    }

    /// Decrypts a fetched record.
    pub fn open(&self, accepted: &Accepted) -> ChangeWire {
        tail::open_record(&self.authority(), &accepted.record).unwrap()
    }

    /// Re-seals a fetched record after `edit`, keeping its sequence.
    pub fn reseal(&self, accepted: &Accepted, edit: impl FnOnce(&mut ChangeWire)) -> Accepted {
        tail::reseal(&self.authority(), accepted, edit).unwrap()
    }

    /// Applies one fetched page through the encrypted tail.
    pub async fn apply(&self, db: &Database, page: &tail::Page) -> Result<()> {
        db.apply_encrypted_tail_page(&self.authority(), page).await
    }

    /// Pulls and applies one page of at most `limit` records. Returns whether
    /// more records remain.
    pub async fn pull_one(&self, db: &Database, limit: usize) -> Result<bool> {
        let page = self.fetch(db, limit).await?;
        self.apply(db, &page).await?;
        Ok(page.has_more)
    }

    /// Pushes every pending change, then pulls until caught up.
    pub async fn sync(&self, db: &Database) -> Result<()> {
        self.sync_with(db, usize::MAX, tail::PAGE_COUNT).await
    }

    /// Alternates pushing up to `push_limit` changes with pulling one page of
    /// at most `pull_limit` records until nothing is pending or remote.
    pub async fn sync_with(
        &self,
        db: &Database,
        push_limit: usize,
        pull_limit: usize,
    ) -> Result<()> {
        loop {
            let mut pushed = 0;
            while pushed < push_limit && self.push_one(db).await? {
                pushed += 1;
            }
            let has_more = self.pull_one(db, pull_limit).await?;
            if !has_more && pushed < push_limit {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::TaskDraft;

    fn draft(title: &str) -> TaskDraft {
        TaskDraft {
            title: title.into(),
            description: String::new(),
            project: Some("app".into()),
            status: "todo".into(),
            priority: "none".into(),
            source: crate::choices::TaskSource::Cli,
            labels: vec![],
            metadata: vec![],
            available_at: None,
            due_on: None,
            is_epic: false,
        }
    }

    #[tokio::test]
    async fn related_comparison_observes_own_acceptance_before_earlier_remote_removal() {
        let root = tempfile::tempdir().unwrap();
        let local = Database::open(&root.path().join("local.sqlite"))
            .await
            .unwrap();
        let remote = Database::open(&root.path().join("remote.sqlite"))
            .await
            .unwrap();
        let server = EncryptedSyncServer::new().await;
        let workspace = local.list_workspaces().await.unwrap().remove(0);
        let task = local
            .create_task(&workspace, draft("a"))
            .await
            .unwrap()
            .task;
        let related = local
            .create_task(&workspace, draft("b"))
            .await
            .unwrap()
            .task;
        local
            .add_task_related_link(&workspace, &task.id, &related.id)
            .await
            .unwrap();
        server.sync(&local).await.unwrap();
        server.sync(&remote).await.unwrap();

        remote
            .remove_task_related_link(&workspace, &task.id, &related.id)
            .await
            .unwrap();
        server.sync(&remote).await.unwrap();
        local
            .remove_task_related_link(&workspace, &task.id, &related.id)
            .await
            .unwrap();
        let readd = local
            .add_task_related_link(&workspace, &task.id, &related.id)
            .await
            .unwrap()
            .change_id
            .unwrap();
        // Both local commands are accepted after the remote removal, before
        // this replica pulls it.
        while server.push_one(&local).await.unwrap() {}
        let page = server.fetch(&local, tail::PAGE_COUNT).await.unwrap();
        assert_eq!(page.records.len(), 3);
        server.apply(&local, &page).await.unwrap();

        let mut conn = local.acquire_reader().await.unwrap();
        let state: (i64, String) = sqlx::query_as(
            "SELECT linked, last_change_id FROM task_related_links
             WHERE workspace_id = ?",
        )
        .bind(&workspace.id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(state, (1, readd));
        drop(conn);
        server.sync(&remote).await.unwrap();
        let mut conn = remote.acquire_reader().await.unwrap();
        let linked: i64 = sqlx::query_scalar("SELECT linked FROM task_related_links")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(linked, 1);
    }
}
