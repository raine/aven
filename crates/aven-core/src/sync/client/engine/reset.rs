//! Returning a database to local-only use.
//!
//! Reset is local: it contacts no server and changes no membership. Other
//! devices still list this one until they remove it.
use anyhow::{Result, bail};

use super::{LocalPhase, local_phase, unix_now};
use crate::db::Database;
use crate::db::installation::InstallationGuard;
use crate::sync::client::coordination;
use crate::sync::client::host::{ClientHost, key_store};

const SETUP_IN_PROGRESS: &str = "error sync-reset-setup-in-progress hint=\"finish setup by rerunning `aven sync setup`; if the original setup can no longer be resumed, run `aven sync reset --force`\"";
const JOIN_IN_PROGRESS: &str = "error sync-reset-join-in-progress hint=\"finish joining by rerunning `aven sync join` before resetting\"";
const INVITATION_OPEN: &str = "error sync-reset-invitation-open hint=\"wait for the other device to join, or run `aven sync invite --cancel`, before resetting\"";

/// What a reset found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reset {
    /// The database took part in sync and no longer does.
    Reset,
    /// The database didn't take part in sync; leftover sync state, if any,
    /// was removed.
    NotSetUp,
}

/// Makes `database` a local database that never synced: sync state, the
/// outbox and this database's protected keys are deleted, while tasks, images
/// and history stay. Setup and join then apply as for a new database.
/// Refused while joining or an unexpired invitation is unfinished. An
/// unfinished setup requires explicit permission because abandoning one may
/// orphan server state when its claim succeeded but its response was lost.
pub async fn reset(
    database: &Database,
    host: &dyn ClientHost,
    force_incomplete_setup: bool,
) -> Result<Reset> {
    let _guard = coordination::acquire(database).await?;
    let phase = local_phase(database).await?;
    match phase {
        LocalPhase::SetupIncomplete if !force_incomplete_setup => bail!(SETUP_IN_PROGRESS),
        LocalPhase::SetupIncomplete => {}
        LocalPhase::JoinIncomplete => bail!(JOIN_IN_PROGRESS),
        LocalPhase::SetUp if invitation_open(database, host).await => bail!(INVITATION_OPEN),
        LocalPhase::SetUp | LocalPhase::NotSetUp => {}
    }
    let store = key_store(host, database).await?;
    let installation = database
        .file_identity()
        .map(InstallationGuard::acquire)
        .transpose()?;
    // Local state goes first, so an interruption leaves a local database
    // whose remaining keys and fence a rerun removes.
    database.clear_sync_state().await?;
    store.erase()?;
    if let Some(installation) = installation {
        installation.unfence()?;
    }
    Ok(match phase {
        LocalPhase::SetupIncomplete | LocalPhase::SetUp => Reset::Reset,
        LocalPhase::JoinIncomplete | LocalPhase::NotSetUp => Reset::NotSetUp,
    })
}

/// An unexpired invitation that the other device may still complete. Keys
/// that can't be read can't admit a device either, so failures count as none.
async fn invitation_open(database: &Database, host: &dyn ClientHost) -> bool {
    let open = async {
        let store = key_store(host, database).await?;
        let Some((_, server)) = store.association(database).await? else {
            return anyhow::Ok(false);
        };
        let inputs = store.active_inputs(database, &server).await?;
        let now = unix_now()?;
        Ok(store
            .open_invitation(database, &inputs)
            .await?
            .is_some_and(|invitation| invitation.expires_at > now))
    };
    open.await.unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use super::*;
    use crate::api::{CreateTask, Store};
    use crate::choices::{TaskPriority, TaskStatus};
    use crate::sync::client::keys::test_support::isolated_store;
    use crate::sync::client::keys::{FileProtectedStorage, ProtectedStorage, StoreResult};

    struct Host(PathBuf);

    impl ClientHost for Host {
        fn ensure_sync_allowed(&self) -> Result<()> {
            Ok(())
        }
        fn protected_storage(&self) -> StoreResult<Arc<dyn ProtectedStorage>> {
            Ok(Arc::new(FileProtectedStorage::new(self.0.clone())))
        }
        fn blob_dir(&self, database: &Database) -> Result<PathBuf> {
            Ok(database.path().with_extension("blobs"))
        }
        fn attachment_lifecycle_policy(&self) -> crate::attachments::LifecyclePolicy {
            crate::attachments::LifecyclePolicy::default()
        }
        fn device_label(&self) -> Option<String> {
            None
        }
    }

    async fn database_with_task(path: &Path) -> Database {
        let store = Store::open(path).await.unwrap();
        let workspace = store.resolve_workspace("default").await.unwrap();
        store
            .create_task(
                &workspace.id,
                CreateTask {
                    title: "Kept through reset".to_string(),
                    description: String::new(),
                    project: "app".to_string(),
                    status: Some(TaskStatus::Todo),
                    priority: TaskPriority::None,
                    metadata: Vec::new(),
                    available_at: None,
                    due_on: None,
                },
            )
            .await
            .unwrap();
        drop(store);
        Database::open(path).await.unwrap()
    }

    async fn count(database: &Database, sql: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .fetch_one(database.pool())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn reset_keeps_data_and_leaves_a_database_that_can_set_up_again() {
        let directory = tempfile::tempdir().unwrap();
        let keys = directory.path().join("keys");
        let path = directory.path().join("aven.sqlite");
        let database = database_with_task(&path).await;
        let host = Host(keys.clone());

        // Leftover sync state: protected keys, the fence, and local sync rows.
        isolated_store(&database, &keys)
            .await
            .load_or_create()
            .unwrap();
        InstallationGuard::acquire(&path).unwrap().fence().unwrap();
        sqlx::query(
            "INSERT OR REPLACE INTO meta(key, value) VALUES
             ('e2ee_association', 'old'), ('sync_cursor', '42'), ('sync_generation', '3')",
        )
        .execute(database.pool())
        .await
        .unwrap();
        let key_files = || {
            std::fs::read_dir(&keys)
                .unwrap()
                .filter_map(|entry| entry.unwrap().file_name().into_string().ok())
                .filter(|name| !name.ends_with(".lock"))
                .count()
        };
        assert!(key_files() > 0);

        assert_eq!(
            reset(&database, &host, false).await.unwrap(),
            Reset::NotSetUp
        );

        assert_eq!(count(&database, "SELECT COUNT(*) FROM tasks").await, 1);
        assert!(count(&database, "SELECT COUNT(*) FROM changes").await > 0);
        assert_eq!(key_files(), 0);
        assert!(
            InstallationGuard::acquire(&path)
                .unwrap()
                .ensure_unbound()
                .is_ok()
        );
        assert_eq!(database.meta("e2ee_association").await.unwrap(), None);
        assert_eq!(
            database.meta("sync_cursor").await.unwrap().as_deref(),
            Some("0")
        );
        assert_eq!(local_phase(&database).await.unwrap(), LocalPhase::NotSetUp);
        super::super::ensure_setup_available(&database, &host)
            .await
            .unwrap();
        // Setup would create a new keyring rather than find a missing one.
        isolated_store(&database, &keys)
            .await
            .load_or_create()
            .unwrap();

        // Rerunning is harmless.
        assert_eq!(
            reset(&database, &host, false).await.unwrap(),
            Reset::NotSetUp
        );
        assert_eq!(count(&database, "SELECT COUNT(*) FROM tasks").await, 1);
    }
}
