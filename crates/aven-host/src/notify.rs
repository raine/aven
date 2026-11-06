//! Delivering queued notifications to the platform's notification service.
//!
//! Notifications are queued in the database's outbox and delivered by one
//! process at a time, the holder of `<db>.delivery.lock`. A running Aven
//! Desktop holds it and posts from its own process. Otherwise the host posts.
//!
//! On macOS, every process that talks to `UNUserNotificationCenter` under
//! Aven's bundle identifier receives clicks on Aven's notifications, so a
//! long-lived process other than the app never talks to it. It runs a
//! short-lived child of the bundle executable for each delivery instead
//! ([`child::ChildProcess`]), which exits before the user can click.
use std::collections::HashSet;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use aven_core::db::Database;
use aven_core::notification_outbox::{OutboxRow, OutboxState, QueuedNotification};
use serde::{Deserialize, Serialize};

pub mod child;
#[cfg(target_os = "macos")]
pub mod un;

/// Why rows failed while the user has notifications turned off for Aven or
/// was never asked.
pub const NOTIFICATIONS_OFF: &str = "notifications-off";
/// Why rows failed in a process with no notification settings, such as an
/// app run outside its bundle.
pub const UNAVAILABLE: &str = "notifications-unavailable";
/// How long a process asking the user to allow notifications waits for the
/// answer.
pub const ASK_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// What the user allowed for Aven's notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Authorization {
    Allowed,
    Denied,
    /// Aven hasn't asked yet. Posts are dropped silently until it does.
    NotAsked,
    /// The process has no notification settings to read.
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Post {
    pub tag: String,
    pub title: String,
    pub body: String,
}

/// One delivery: read the authorization, list delivered notifications if
/// asked, then post, but only when notifications are allowed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub posts: Vec<Post>,
    pub list_delivered: bool,
    /// Ask the user to allow notifications when they haven't been asked,
    /// and wait for the answer. A process that exits before the user answers
    /// withdraws the question and leaves notifications denied, so only a
    /// process the user is waiting on asks.
    #[serde(default)]
    pub ask: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub authorization: Authorization,
    /// For each post in order, the error the service returned, if any.
    pub errors: Vec<Option<String>>,
    /// Tags of notifications the service shows, when asked and it can tell.
    pub delivered: Option<Vec<String>>,
}

impl Report {
    pub fn not_allowed(authorization: Authorization) -> Self {
        Self {
            authorization,
            errors: Vec::new(),
            delivered: None,
        }
    }
}

/// Hands notifications to the platform's notification service.
pub trait Deliver: Send + Sync {
    /// Blocks until the service answered. An error means the outcome of the
    /// posts is unknown.
    fn deliver(&self, request: &Request) -> Result<Report>;
}

/// What a click on a notification opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Task {
        workspace: String,
        task: String,
    },
    /// The tasks due on and before this date, in this workspace or, for a
    /// digest tagged before digests were per workspace, the current one.
    Due {
        workspace: Option<String>,
        date: String,
    },
}

/// The tag of a notification about one task. `event` keeps it apart from
/// earlier notifications about the same task.
pub fn task_tag(workspace: &str, task: &str, event: &str) -> String {
    format!("v1:task:{workspace}:{task}:{event}")
}

/// The tag of a workspace's due digest for a date (`YYYY-MM-DD`).
pub fn digest_tag(workspace: &str, date: &str) -> String {
    format!("v1:digest:{workspace}:{date}")
}

/// The tag of a notification with nothing to open but the app.
pub fn plain_tag(kind: &str, event: &str) -> String {
    format!("v1:{kind}:{event}")
}

/// What a notification's tag opens. Accepts `task:{workspace}:{task}` from
/// versions before the `v1` tags, and `v1:digest:{date}` from before digests
/// were per workspace.
pub fn target(tag: &str) -> Option<Target> {
    if let Some(rest) = tag.strip_prefix("v1:task:") {
        let (rest, _event) = rest.rsplit_once(':')?;
        return task_target(rest);
    }
    if let Some(rest) = tag.strip_prefix("v1:digest:") {
        let (workspace, date) = match rest.rsplit_once(':') {
            Some((workspace, date)) => (Some(workspace.to_string()), date),
            None => (None, rest),
        };
        return (!date.is_empty() && workspace.as_deref() != Some("")).then(|| Target::Due {
            workspace,
            date: date.to_string(),
        });
    }
    task_target(tag.strip_prefix("task:")?)
}

fn task_target(rest: &str) -> Option<Target> {
    let (workspace, task) = rest.rsplit_once(':')?;
    (!workspace.is_empty() && !task.is_empty()).then(|| Target::Task {
        workspace: workspace.to_string(),
        task: task.to_string(),
    })
}

/// A new event id for a queued notification.
pub fn new_event() -> String {
    let mut bytes = [0_u8; 8];
    if getrandom::fill(&mut bytes).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        bytes = (nanos as u64).to_be_bytes();
    }
    hex::encode(bytes)
}

/// What the user allowed, read through `deliver` without posting.
pub fn authorization(deliver: &dyn Deliver) -> Result<Authorization> {
    Ok(deliver.deliver(&Request::default())?.authorization)
}

/// Asks the user to allow notifications unless they already answered, and
/// waits for the answer. Returns what they allowed.
pub fn ask(deliver: &dyn Deliver) -> Result<Authorization> {
    Ok(deliver
        .deliver(&Request {
            ask: true,
            ..Request::default()
        })?
        .authorization)
}

pub fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Where the delivery lock of the database at `db` lives.
pub fn lock_path(db: &Path) -> PathBuf {
    let mut name = db.file_name().unwrap_or(db.as_os_str()).to_os_string();
    name.push(".delivery.lock");
    db.with_file_name(name)
}

/// The right to deliver a database's notifications. Dropping it gives it up.
#[derive(Debug)]
pub struct DeliveryLock(#[allow(dead_code)] File);

impl DeliveryLock {
    /// Takes the lock unless another process holds it.
    pub fn try_take(db: &Path) -> Result<Option<Self>> {
        let path = lock_path(db);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("open delivery lock {}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self(file))),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => {
                Err(error).with_context(|| format!("lock {}", path.display()))
            }
        }
    }
}

/// What one delivery did.
#[derive(Debug, Default)]
pub struct Drained {
    pub submitted: Vec<QueuedNotification>,
    pub failed: Vec<(QueuedNotification, String)>,
    /// Undelivered rows that expired and were dropped.
    pub expired: u64,
}

/// Delivers the database's waiting notifications if no other process
/// delivers them. Does nothing, and touches no notification service, when
/// none wait.
pub async fn deliver_pending(database: &Database, deliver: &Arc<dyn Deliver>) -> Result<Drained> {
    if !database.outbox_has_work(unix_now()).await? {
        return Ok(Drained::default());
    }
    let Some(_lock) = DeliveryLock::try_take(database.path())? else {
        return Ok(Drained::default());
    };
    drain(database, deliver).await
}

/// Delivers the outbox's waiting rows. The caller holds the delivery lock.
///
/// A row left `submitting` by an earlier delivery may already show, so it is
/// posted again only if the service doesn't list it. A service that can't
/// list what it shows gets it again, which can repeat it.
pub async fn drain(database: &Database, deliver: &Arc<dyn Deliver>) -> Result<Drained> {
    let now = unix_now();
    let mut drained = Drained {
        expired: database.drop_expired_notifications(now).await?,
        ..Drained::default()
    };
    let waiting = database.outbox_waiting(now).await?;
    if waiting.is_empty() {
        return Ok(drained);
    }
    let (unsure, mut posts): (Vec<OutboxRow>, Vec<OutboxRow>) = waiting
        .into_iter()
        .partition(|row| row.state == OutboxState::Submitting);
    if !unsure.is_empty() {
        let report = call(
            deliver,
            Request {
                list_delivered: true,
                ..Request::default()
            },
        )
        .await?;
        if let Some(reason) = refusal(report.authorization) {
            let rows: Vec<OutboxRow> = unsure.into_iter().chain(posts).collect();
            fail(database, &mut drained, rows, reason).await?;
            return Ok(drained);
        }
        let delivered: HashSet<String> = report.delivered.unwrap_or_default().into_iter().collect();
        let (shown, again): (Vec<OutboxRow>, Vec<OutboxRow>) = unsure
            .into_iter()
            .partition(|row| delivered.contains(&row.notification.tag));
        database
            .set_outbox_state(&event_ids(&shown), &OutboxState::Submitted, unix_now())
            .await?;
        drained
            .submitted
            .extend(shown.into_iter().map(|row| row.notification));
        posts.splice(0..0, again);
    }
    if posts.is_empty() {
        return Ok(drained);
    }
    database
        .set_outbox_state(&event_ids(&posts), &OutboxState::Submitting, unix_now())
        .await?;
    let report = call(
        deliver,
        Request {
            posts: posts
                .iter()
                .map(|row| Post {
                    tag: row.notification.tag.clone(),
                    title: row.notification.title.clone(),
                    body: row.notification.body.clone(),
                })
                .collect(),
            ..Request::default()
        },
    )
    .await?;
    if let Some(reason) = refusal(report.authorization) {
        fail(database, &mut drained, posts, reason).await?;
        return Ok(drained);
    }
    // A post without an answer stays `submitting`, for the next delivery to
    // check.
    for (row, error) in posts.into_iter().zip(report.errors) {
        let state = match &error {
            None => OutboxState::Submitted,
            Some(error) => OutboxState::Failed(error.clone()),
        };
        database
            .set_outbox_state(
                std::slice::from_ref(&row.notification.event),
                &state,
                unix_now(),
            )
            .await?;
        match error {
            None => drained.submitted.push(row.notification),
            Some(error) => drained.failed.push((row.notification, error)),
        }
    }
    Ok(drained)
}

async fn call(deliver: &Arc<dyn Deliver>, request: Request) -> Result<Report> {
    let deliver = deliver.clone();
    tokio::task::spawn_blocking(move || deliver.deliver(&request)).await?
}

fn refusal(authorization: Authorization) -> Option<&'static str> {
    match authorization {
        Authorization::Allowed => None,
        Authorization::Denied | Authorization::NotAsked => Some(NOTIFICATIONS_OFF),
        Authorization::Unavailable => Some(UNAVAILABLE),
    }
}

async fn fail(
    database: &Database,
    drained: &mut Drained,
    rows: Vec<OutboxRow>,
    reason: &str,
) -> Result<()> {
    database
        .set_outbox_state(
            &event_ids(&rows),
            &OutboxState::Failed(reason.to_string()),
            unix_now(),
        )
        .await?;
    drained.failed.extend(
        rows.into_iter()
            .map(|row| (row.notification, reason.to_string())),
    );
    Ok(())
}

fn event_ids(rows: &[OutboxRow]) -> Vec<String> {
    rows.iter()
        .map(|row| row.notification.event.clone())
        .collect()
}

#[cfg(test)]
mod tests;
