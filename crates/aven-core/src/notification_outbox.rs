//! Notifications this device has queued for the platform's notification
//! service. The table is local: it never syncs.
//!
//! A row is `pending` until a deliverer takes it, `submitting` while the
//! service is being handed it, then `submitted` or `failed` with a reason. A
//! deliverer that stops while a row is `submitting` leaves it unknown whether
//! the service got it; the next one checks the service's delivered
//! notifications before posting it again.
use anyhow::Result;

use crate::db::{Database, begin_immediate};
use crate::local_state::{ReminderLogEntry, record_reminder_entries};

/// How long a submitted or failed row is kept, for status shown to the user.
pub const FINISHED_KEEP_SECONDS: i64 = 7 * 24 * 60 * 60;

/// A notification to queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedNotification {
    /// Unique per notification.
    pub event: String,
    /// What it is about, such as `available` or `digest`.
    pub kind: String,
    /// The platform's identifier for it, which a click hands back.
    pub tag: String,
    pub title: String,
    pub body: String,
    /// Unix seconds after which it is dropped rather than shown late.
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboxState {
    Pending,
    Submitting,
    Submitted,
    Failed(String),
}

impl OutboxState {
    fn name(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Submitting => "submitting",
            Self::Submitted => "submitted",
            Self::Failed(_) => "failed",
        }
    }

    fn reason(&self) -> Option<&str> {
        match self {
            Self::Failed(reason) => Some(reason),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxRow {
    pub notification: QueuedNotification,
    pub state: OutboxState,
    pub created_at: i64,
}

#[derive(sqlx::FromRow)]
struct Row {
    event: String,
    kind: String,
    tag: String,
    title: String,
    body: String,
    created_at: i64,
    expires_at: i64,
    state: String,
    reason: Option<String>,
}

impl From<Row> for OutboxRow {
    fn from(row: Row) -> Self {
        let state = match row.state.as_str() {
            "pending" => OutboxState::Pending,
            "submitting" => OutboxState::Submitting,
            "submitted" => OutboxState::Submitted,
            _ => OutboxState::Failed(row.reason.unwrap_or_default()),
        };
        Self {
            notification: QueuedNotification {
                event: row.event,
                kind: row.kind,
                tag: row.tag,
                title: row.title,
                body: row.body,
                expires_at: row.expires_at,
            },
            state,
            created_at: row.created_at,
        }
    }
}

impl Database {
    /// Records reminder log entries and queues notifications in one
    /// transaction, so a reminder is never recorded without its notification
    /// or queued twice.
    pub async fn queue_notifications(
        &self,
        log: &[ReminderLogEntry],
        notifications: &[QueuedNotification],
        now: i64,
    ) -> Result<()> {
        if log.is_empty() && notifications.is_empty() {
            return Ok(());
        }
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        record_reminder_entries(&mut tx, log).await?;
        for notification in notifications {
            sqlx::query(
                "INSERT OR IGNORE INTO local_notification_outbox
                     (event, kind, tag, title, body, created_at, expires_at, state, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, 'pending', ?)",
            )
            .bind(&notification.event)
            .bind(&notification.kind)
            .bind(&notification.tag)
            .bind(&notification.title)
            .bind(&notification.body)
            .bind(now)
            .bind(notification.expires_at)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Whether any unexpired row waits for delivery or a check.
    pub async fn outbox_has_work(&self, now: i64) -> Result<bool> {
        let mut conn = self.acquire_reader().await?;
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS (SELECT 1 FROM local_notification_outbox
                            WHERE state IN ('pending', 'submitting') AND expires_at > ?)",
        )
        .bind(now)
        .fetch_one(&mut *conn)
        .await?
            != 0)
    }

    /// Unexpired `pending` and `submitting` rows, oldest first.
    pub async fn outbox_waiting(&self, now: i64) -> Result<Vec<OutboxRow>> {
        let mut conn = self.acquire_reader().await?;
        let rows = sqlx::query_as::<_, Row>(
            "SELECT event, kind, tag, title, body, created_at, expires_at, state, reason
             FROM local_notification_outbox
             WHERE state IN ('pending', 'submitting') AND expires_at > ?
             ORDER BY created_at, event",
        )
        .bind(now)
        .fetch_all(&mut *conn)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn set_outbox_state(
        &self,
        events: &[String],
        state: &OutboxState,
        now: i64,
    ) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        for event in events {
            sqlx::query(
                "UPDATE local_notification_outbox
                 SET state = ?, reason = ?, updated_at = ?
                 WHERE event = ?",
            )
            .bind(state.name())
            .bind(state.reason())
            .bind(now)
            .bind(event)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Deletes rows that expired before delivery and finished rows older than
    /// [`FINISHED_KEEP_SECONDS`]. Returns how many undelivered rows expired.
    pub async fn drop_expired_notifications(&self, now: i64) -> Result<u64> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let expired = sqlx::query(
            "DELETE FROM local_notification_outbox
             WHERE state IN ('pending', 'submitting') AND expires_at <= ?",
        )
        .bind(now)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        sqlx::query(
            "DELETE FROM local_notification_outbox
             WHERE state IN ('submitted', 'failed') AND updated_at <= ?",
        )
        .bind(now - FINISHED_KEEP_SECONDS)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(expired)
    }

    /// How many rows failed for `reason` at or after `since`.
    pub async fn failed_notifications_since(&self, reason: &str, since: i64) -> Result<u64> {
        let mut conn = self.acquire_reader().await?;
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM local_notification_outbox
             WHERE state = 'failed' AND reason = ? AND updated_at >= ?",
        )
        .bind(reason)
        .bind(since)
        .fetch_one(&mut *conn)
        .await?;
        Ok(u64::try_from(count).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn database() -> Database {
        Database::open(std::path::Path::new(":memory:"))
            .await
            .unwrap()
    }

    fn queued(event: &str, expires_at: i64) -> QueuedNotification {
        QueuedNotification {
            event: event.into(),
            kind: "available".into(),
            tag: format!("v1:task:home:T:{event}"),
            title: "Available APP-1".into(),
            body: "Write the report".into(),
            expires_at,
        }
    }

    fn entry(outcome: &str) -> ReminderLogEntry {
        ReminderLogEntry {
            kind: "available".into(),
            task_id: "T".into(),
            trigger: "2026-11-06T09:00:00Z".into(),
            outcome: outcome.into(),
        }
    }

    fn events(rows: &[OutboxRow]) -> Vec<&str> {
        rows.iter()
            .map(|row| row.notification.event.as_str())
            .collect()
    }

    #[tokio::test]
    async fn queueing_records_the_reminder_with_its_notification() {
        let database = database().await;
        database
            .queue_notifications(&[entry("sent")], &[queued("a", 200)], 100)
            .await
            .unwrap();

        assert_eq!(database.reminder_log().await.unwrap(), vec![entry("sent")]);
        let waiting = database.outbox_waiting(100).await.unwrap();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].notification, queued("a", 200));
        assert_eq!(waiting[0].state, OutboxState::Pending);
        assert_eq!(waiting[0].created_at, 100);
    }

    #[tokio::test]
    async fn a_queued_event_is_never_queued_again() {
        let database = database().await;
        database
            .queue_notifications(&[], &[queued("a", 200)], 100)
            .await
            .unwrap();
        database
            .set_outbox_state(&["a".into()], &OutboxState::Submitted, 110)
            .await
            .unwrap();
        database
            .queue_notifications(&[], &[queued("a", 300)], 120)
            .await
            .unwrap();

        assert!(database.outbox_waiting(120).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn only_unexpired_pending_and_submitting_rows_wait() {
        let database = database().await;
        let rows = [
            queued("pending", 200),
            queued("submitting", 200),
            queued("submitted", 200),
            queued("failed", 200),
            queued("expired", 150),
        ];
        database.queue_notifications(&[], &rows, 100).await.unwrap();
        for (event, state) in [
            ("submitting", OutboxState::Submitting),
            ("submitted", OutboxState::Submitted),
            ("failed", OutboxState::Failed("notifications-off".into())),
        ] {
            database
                .set_outbox_state(&[event.into()], &state, 100)
                .await
                .unwrap();
        }

        assert_eq!(
            events(&database.outbox_waiting(160).await.unwrap()),
            ["pending", "submitting"]
        );
        assert!(database.outbox_has_work(160).await.unwrap());
        assert!(!database.outbox_has_work(200).await.unwrap());
    }

    #[tokio::test]
    async fn expired_rows_are_dropped_and_finished_rows_kept_for_a_while() {
        let database = database().await;
        database
            .queue_notifications(
                &[],
                &[
                    queued("late", 150),
                    queued("done", 150),
                    queued("next", 500),
                ],
                100,
            )
            .await
            .unwrap();
        database
            .set_outbox_state(&["done".into()], &OutboxState::Submitted, 120)
            .await
            .unwrap();

        assert_eq!(database.drop_expired_notifications(160).await.unwrap(), 1);
        assert_eq!(
            events(&database.outbox_waiting(160).await.unwrap()),
            ["next"]
        );
        database
            .set_outbox_state(
                &["next".into()],
                &OutboxState::Failed("notifications-off".into()),
                130,
            )
            .await
            .unwrap();
        assert_eq!(
            database
                .failed_notifications_since("notifications-off", 100)
                .await
                .unwrap(),
            1
        );

        let later = 130 + FINISHED_KEEP_SECONDS;
        assert_eq!(database.drop_expired_notifications(later).await.unwrap(), 0);
        assert_eq!(
            database
                .failed_notifications_since("notifications-off", 0)
                .await
                .unwrap(),
            0
        );
    }
}
