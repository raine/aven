use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;

use crate::db::{Database, begin_immediate, get_meta, set_meta};
use crate::ids::WorkspaceId;

const IOS_QUEUE_WORKSPACE_META_KEY: &str = "ios_queue_workspace_id";
const ONBOARDING_META_KEY: &str = "tui_onboarding_version";
const SYNC_ACCESS_REFUSED_AT_META_KEY: &str = "sync_access_refused_at";
/// The `sync_` prefix places it in the sync state that reset clears.
const SYNC_VAULT_DELETED_AT_META_KEY: &str = "sync_vault_deleted_at";
const FIRST_LAUNCH_ONBOARDING_VERSION: u32 = 1;
const TRIAGE_REVIEWS_META_KEY: &str = "tui_triage_reviews";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncAccessRefusal {
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncVaultDeletion {
    pub at: String,
}

/// Local daemon health, cleared with the other sync metadata on reset.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DaemonSyncState {
    /// Unix seconds of the last completed daemon round.
    pub last_success_at: Option<i64>,
    pub error: Option<DaemonSyncError>,
    /// Unix seconds of the backoff deadline, absent after success.
    pub next_retry_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DaemonSyncError {
    pub code: String,
    pub message: String,
    /// Unix seconds when this error code first appeared without a success.
    pub since: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingStatus {
    Due,
    Complete,
    Established,
}

impl Database {
    pub async fn daemon_sync_state(&self) -> Result<DaemonSyncState> {
        self.meta("sync_daemon_health")
            .await?
            .map(|value| serde_json::from_str(&value).map_err(Into::into))
            .unwrap_or_else(|| Ok(DaemonSyncState::default()))
    }

    /// Records an actual round, not idle polling or lock contention. Repeated
    /// errors retain their first-seen time; unchanged snapshots cause no write.
    pub async fn record_daemon_sync_round(
        &self,
        at: i64,
        error: Option<(&str, &str, i64)>,
    ) -> Result<()> {
        let previous = self.daemon_sync_state().await?;
        let mut state = previous.clone();
        if let Some((code, message, retry_at)) = error {
            let since = previous
                .error
                .as_ref()
                .filter(|error| error.code == code)
                .map_or(at, |error| error.since);
            state.error = Some(DaemonSyncError {
                code: code.to_string(),
                message: message.to_string(),
                since,
            });
            state.next_retry_at = Some(retry_at);
        } else {
            state.last_success_at = Some(at);
            state.error = None;
            state.next_retry_at = None;
        }
        if state != previous {
            let mut conn = self.acquire_writer().await?;
            set_meta(
                &mut conn,
                "sync_daemon_health",
                &serde_json::to_string(&state)?,
            )
            .await?;
        }
        Ok(())
    }

    /// Returns the last time this installation's sync credentials were refused.
    pub async fn sync_access_refusal(&self) -> Result<Option<SyncAccessRefusal>> {
        Ok(self
            .meta(SYNC_ACCESS_REFUSED_AT_META_KEY)
            .await?
            .map(|at| SyncAccessRefusal { at }))
    }

    /// Remembers an authentication refusal until a later sync succeeds.
    pub async fn record_sync_access_refusal(&self) -> Result<SyncAccessRefusal> {
        let at = crate::ids::now();
        let mut conn = self.acquire_writer().await?;
        set_meta(&mut conn, SYNC_ACCESS_REFUSED_AT_META_KEY, &at).await?;
        Ok(SyncAccessRefusal { at })
    }

    /// Clears a previous authentication refusal after a successful sync.
    pub async fn clear_sync_access_refusal(&self) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        sqlx::query("DELETE FROM meta WHERE key = ?")
            .bind(SYNC_ACCESS_REFUSED_AT_META_KEY)
            .execute(&mut *conn)
            .await?;
        Ok(())
    }

    /// Returns when the server last reported this sync's vault deleted.
    pub async fn sync_vault_deletion(&self) -> Result<Option<SyncVaultDeletion>> {
        Ok(self
            .meta(SYNC_VAULT_DELETED_AT_META_KEY)
            .await?
            .map(|at| SyncVaultDeletion { at }))
    }

    /// Remembers a vault deletion until a user-started sync succeeds or sync
    /// is reset.
    pub async fn record_sync_vault_deletion(&self) -> Result<SyncVaultDeletion> {
        let at = crate::ids::now();
        let mut conn = self.acquire_writer().await?;
        set_meta(&mut conn, SYNC_VAULT_DELETED_AT_META_KEY, &at).await?;
        Ok(SyncVaultDeletion { at })
    }

    pub async fn clear_sync_vault_deletion(&self) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        sqlx::query("DELETE FROM meta WHERE key = ?")
            .bind(SYNC_VAULT_DELETED_AT_META_KEY)
            .execute(&mut *conn)
            .await?;
        Ok(())
    }

    pub async fn restore_queue_workspace(&self) -> Result<crate::workspaces::Workspace> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let workspaces = crate::workspaces::list_workspaces(&mut tx).await?;
        let stored_id = get_meta(&mut tx, IOS_QUEUE_WORKSPACE_META_KEY)
            .await?
            .and_then(|value| value.parse::<WorkspaceId>().ok());
        let selected = stored_id
            .and_then(|id| workspaces.iter().find(|workspace| workspace.id == id))
            .or_else(|| {
                workspaces.iter().find(|workspace| {
                    workspace.id.as_str() == crate::workspaces::DEFAULT_WORKSPACE_ID
                })
            })
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no available workspace"))?;
        set_meta(&mut tx, IOS_QUEUE_WORKSPACE_META_KEY, selected.id.as_str()).await?;
        tx.commit().await?;
        Ok(selected)
    }

    pub async fn select_queue_workspace(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<crate::workspaces::Workspace> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let workspace = crate::workspaces::workspace_for_id(&mut tx, workspace_id).await?;
        set_meta(&mut tx, IOS_QUEUE_WORKSPACE_META_KEY, workspace.id.as_str()).await?;
        tx.commit().await?;
        Ok(workspace)
    }

    pub async fn onboarding_status(&self) -> Result<OnboardingStatus> {
        let mut conn = self.acquire_reader().await?;
        let marker = get_meta(&mut conn, ONBOARDING_META_KEY).await?;
        if marker_version(marker.as_deref())
            .is_some_and(|version| version >= FIRST_LAUNCH_ONBOARDING_VERSION)
        {
            return Ok(OnboardingStatus::Complete);
        }

        let has_tasks: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tasks)")
            .fetch_one(&mut *conn)
            .await?;
        Ok(if has_tasks {
            OnboardingStatus::Established
        } else {
            OnboardingStatus::Due
        })
    }

    pub async fn complete_onboarding(&self) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let marker = get_meta(&mut tx, ONBOARDING_META_KEY).await?;
        if marker_version(marker.as_deref())
            .is_none_or(|version| version < FIRST_LAUNCH_ONBOARDING_VERSION)
        {
            set_meta(
                &mut tx,
                ONBOARDING_META_KEY,
                &FIRST_LAUNCH_ONBOARDING_VERSION.to_string(),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

/// A reminder this device has handled, keyed by kind, task and trigger.
/// Digest entries use an empty `task_id`.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ReminderLogEntry {
    pub kind: String,
    pub task_id: String,
    pub trigger: String,
    pub outcome: String,
}

impl Database {
    pub async fn reminder_log(&self) -> Result<Vec<ReminderLogEntry>> {
        let mut conn = self.acquire_reader().await?;
        Ok(sqlx::query_as::<_, ReminderLogEntry>(
            "SELECT kind, task_id, trigger, outcome FROM local_reminder_log",
        )
        .fetch_all(&mut *conn)
        .await?)
    }

    /// Records entries in one transaction, replacing the outcome of existing
    /// keys.
    pub async fn record_reminders(&self, entries: &[ReminderLogEntry]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let recorded_at = crate::ids::now();
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        for entry in entries {
            sqlx::query(
                "INSERT INTO local_reminder_log(kind, task_id, trigger, outcome, recorded_at)
                 VALUES (?, ?, ?, ?, ?)
                 ON CONFLICT(kind, task_id, trigger)
                 DO UPDATE SET outcome = excluded.outcome, recorded_at = excluded.recorded_at",
            )
            .bind(&entry.kind)
            .bind(&entry.task_id)
            .bind(&entry.trigger)
            .bind(&entry.outcome)
            .bind(&recorded_at)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

fn marker_version(value: Option<&str>) -> Option<u32> {
    value?.trim().parse().ok()
}

/// Database-local sidebar preferences shared across workspaces and projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SidebarSection {
    Views,
    Workspaces,
    Projects,
}

impl SidebarSection {
    pub fn label(self) -> &'static str {
        match self {
            Self::Views => "Views",
            Self::Workspaces => "Workspaces",
            Self::Projects => "Projects",
        }
    }

    fn meta_key(self) -> &'static str {
        match self {
            Self::Views => "tui_sidebar_views_collapsed",
            Self::Workspaces => "tui_sidebar_scope_collapsed",
            Self::Projects => "tui_sidebar_projects_collapsed",
        }
    }
}

impl Database {
    pub async fn collapsed_sidebar_sections(&self) -> Result<BTreeSet<SidebarSection>> {
        let mut conn = self.acquire_reader().await?;
        let mut sections = BTreeSet::new();
        for section in [
            SidebarSection::Views,
            SidebarSection::Workspaces,
            SidebarSection::Projects,
        ] {
            if get_meta(&mut conn, section.meta_key())
                .await?
                .as_deref()
                .map(str::trim)
                == Some("true")
            {
                sections.insert(section);
            }
        }
        Ok(sections)
    }

    pub async fn set_sidebar_section_collapsed(
        &self,
        section: SidebarSection,
        collapsed: bool,
    ) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        set_meta(
            &mut tx,
            section.meta_key(),
            if collapsed { "true" } else { "false" },
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

/// When each task was last kept as it stands in triage, in unix seconds. The
/// marks are local to this database: they only quiet triage on this machine.
pub type TriageReviews = BTreeMap<String, i64>;

impl Database {
    pub async fn triage_reviews(&self) -> Result<TriageReviews> {
        let mut conn = self.acquire_reader().await?;
        Ok(parse_triage_reviews(
            get_meta(&mut conn, TRIAGE_REVIEWS_META_KEY)
                .await?
                .as_deref(),
        ))
    }

    /// Sets or clears a task's review mark and returns the previous one. Marks
    /// older than the longest stale window no longer affect triage and are
    /// dropped.
    pub async fn set_triage_review(
        &self,
        task_id: &str,
        reviewed_at: Option<i64>,
        now_seconds: i64,
    ) -> Result<Option<i64>> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let mut reviews =
            parse_triage_reviews(get_meta(&mut tx, TRIAGE_REVIEWS_META_KEY).await?.as_deref());
        let previous = match reviewed_at {
            Some(at) => reviews.insert(task_id.to_string(), at),
            None => reviews.remove(task_id),
        };
        let horizon = now_seconds - crate::queue::STALE_TODO_DAYS * 86_400;
        reviews.retain(|_, at| *at > horizon);
        set_meta(
            &mut tx,
            TRIAGE_REVIEWS_META_KEY,
            &serde_json::to_string(&reviews)?,
        )
        .await?;
        tx.commit().await?;
        Ok(previous)
    }
}

fn parse_triage_reviews(value: Option<&str>) -> TriageReviews {
    value
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn daemon_health_preserves_first_seen_and_clears_on_success_and_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("health.sqlite");
        let database = Database::open(&path).await.unwrap();
        assert_eq!(
            database.daemon_sync_state().await.unwrap(),
            DaemonSyncState::default()
        );
        database.record_daemon_sync_round(100, None).await.unwrap();
        database
            .record_daemon_sync_round(110, Some(("network", "retry", 111)))
            .await
            .unwrap();
        database
            .record_daemon_sync_round(120, Some(("network", "retry", 122)))
            .await
            .unwrap();
        let state = database.daemon_sync_state().await.unwrap();
        assert_eq!(state.last_success_at, Some(100));
        assert_eq!(state.error.as_ref().unwrap().since, 110);
        assert_eq!(state.next_retry_at, Some(122));
        sqlx::raw_sql(
            "CREATE TRIGGER reject_health_write BEFORE UPDATE ON meta
            WHEN NEW.key = 'sync_daemon_health'
            BEGIN SELECT RAISE(ABORT, 'redundant health write'); END;",
        )
        .execute(&mut *database.acquire_writer().await.unwrap())
        .await
        .unwrap();
        database
            .record_daemon_sync_round(121, Some(("network", "retry", 122)))
            .await
            .unwrap();
        sqlx::raw_sql("DROP TRIGGER reject_health_write")
            .execute(&mut *database.acquire_writer().await.unwrap())
            .await
            .unwrap();
        assert_eq!(
            Database::open(&path)
                .await
                .unwrap()
                .daemon_sync_state()
                .await
                .unwrap(),
            state
        );
        database
            .record_daemon_sync_round(130, Some(("keys", "unlock", 140)))
            .await
            .unwrap();
        assert_eq!(
            database
                .daemon_sync_state()
                .await
                .unwrap()
                .error
                .unwrap()
                .since,
            130
        );
        database.record_daemon_sync_round(150, None).await.unwrap();
        assert_eq!(
            database.daemon_sync_state().await.unwrap(),
            DaemonSyncState {
                last_success_at: Some(150),
                error: None,
                next_retry_at: None,
            }
        );
        database.clear_sync_state().await.unwrap();
        assert_eq!(
            database.daemon_sync_state().await.unwrap(),
            DaemonSyncState::default()
        );
    }

    #[tokio::test]
    async fn deleted_queue_workspace_falls_back_to_the_renamed_default_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.sqlite");
        let database = Database::open(&path).await.unwrap();
        database.create_workspace("Alpha").await.unwrap();
        database.rename_workspace("default", "Home").await.unwrap();
        let personal = database.create_workspace("Personal").await.unwrap();
        database.select_queue_workspace(&personal.id).await.unwrap();
        crate::test_support::set_established_protocol(&database, 19)
            .await
            .unwrap();
        database
            .delete_workspace(&personal.id, &[], true)
            .await
            .unwrap();
        assert_eq!(
            database.restore_queue_workspace().await.unwrap().key,
            "home"
        );
        database.close().await;
        let reopened = Database::open(&path).await.unwrap();
        assert_eq!(
            reopened.restore_queue_workspace().await.unwrap().key,
            "home"
        );
    }

    #[tokio::test]
    async fn sync_access_refusal_persists_until_cleared() {
        let database = Database::open(std::path::Path::new(":memory:"))
            .await
            .unwrap();
        assert_eq!(database.sync_access_refusal().await.unwrap(), None);

        let recorded = database.record_sync_access_refusal().await.unwrap();
        assert!(!recorded.at.is_empty());
        assert_eq!(
            database.sync_access_refusal().await.unwrap(),
            Some(recorded)
        );

        database.clear_sync_access_refusal().await.unwrap();
        assert_eq!(database.sync_access_refusal().await.unwrap(), None);
    }

    #[tokio::test]
    async fn reminder_outcome_replaces_existing_key() {
        let database = Database::open(std::path::Path::new(":memory:"))
            .await
            .unwrap();
        let entry = |outcome: &str| ReminderLogEntry {
            kind: "available".into(),
            task_id: "T1".into(),
            trigger: "2026-10-14T09:00:00Z".into(),
            outcome: outcome.into(),
        };

        database
            .record_reminders(&[entry("waiting")])
            .await
            .unwrap();
        database.record_reminders(&[entry("sent")]).await.unwrap();

        assert_eq!(database.reminder_log().await.unwrap(), vec![entry("sent")]);
    }

    #[tokio::test]
    async fn sync_vault_deletion_persists_apart_from_access_refusal_until_cleared() {
        let database = Database::open(std::path::Path::new(":memory:"))
            .await
            .unwrap();
        assert_eq!(database.sync_vault_deletion().await.unwrap(), None);

        let recorded = database.record_sync_vault_deletion().await.unwrap();
        assert_eq!(
            database.sync_vault_deletion().await.unwrap(),
            Some(recorded)
        );
        assert_eq!(database.sync_access_refusal().await.unwrap(), None);

        database.clear_sync_vault_deletion().await.unwrap();
        assert_eq!(database.sync_vault_deletion().await.unwrap(), None);
    }

    #[tokio::test]
    async fn sidebar_preferences_are_local_and_invalid_values_default_to_expanded() {
        let database = Database::open(std::path::Path::new(":memory:"))
            .await
            .unwrap();
        let before: i64 = {
            let mut conn = database.acquire_reader().await.unwrap();
            sqlx::query_scalar("SELECT COUNT(*) FROM changes")
                .fetch_one(&mut *conn)
                .await
                .unwrap()
        };
        {
            let mut conn = database.acquire_writer().await.unwrap();
            set_meta(&mut conn, "tui_sidebar_scope_collapsed", "true")
                .await
                .unwrap();
        }
        {
            let mut conn = database.acquire_writer().await.unwrap();
            set_meta(&mut conn, SidebarSection::Views.meta_key(), "invalid")
                .await
                .unwrap();
        }
        assert_eq!(
            database.collapsed_sidebar_sections().await.unwrap(),
            BTreeSet::from([SidebarSection::Workspaces])
        );
        let mut conn = database.acquire_reader().await.unwrap();
        let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM changes")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(before, after);
    }

    #[tokio::test]
    async fn triage_reviews_round_trip_and_drop_expired_marks() {
        let database = Database::open(std::path::Path::new(":memory:"))
            .await
            .unwrap();
        let now = 100 * 86_400;
        database
            .set_triage_review("OLD", Some(now - 31 * 86_400), now)
            .await
            .unwrap();
        assert_eq!(
            database
                .set_triage_review("NEW", Some(now), now)
                .await
                .unwrap(),
            None
        );

        let reviews = database.triage_reviews().await.unwrap();
        assert_eq!(reviews.get("NEW"), Some(&now));
        assert!(!reviews.contains_key("OLD"));

        assert_eq!(
            database.set_triage_review("NEW", None, now).await.unwrap(),
            Some(now)
        );
        assert!(database.triage_reviews().await.unwrap().is_empty());
    }
}
