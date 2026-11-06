use std::sync::Mutex;

use super::*;

/// A notification service that records requests and answers as configured.
struct Service {
    authorization: Authorization,
    /// Tags it lists as delivered, or `None` when it can't tell.
    delivered: Option<Vec<String>>,
    /// Tags whose post the service refuses.
    refuse: Vec<String>,
    /// Whether the call itself fails, as when the child dies.
    broken: bool,
    requests: Mutex<Vec<Request>>,
}

impl Service {
    fn allowing() -> Self {
        Self {
            authorization: Authorization::Allowed,
            delivered: Some(Vec::new()),
            refuse: Vec::new(),
            broken: false,
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}

impl Deliver for Service {
    fn deliver(&self, request: &Request) -> Result<Report> {
        self.requests.lock().unwrap().push(request.clone());
        if self.broken {
            anyhow::bail!("the child died");
        }
        if self.authorization != Authorization::Allowed {
            return Ok(Report::not_allowed(self.authorization));
        }
        Ok(Report {
            authorization: self.authorization,
            errors: request
                .posts
                .iter()
                .map(|post| {
                    self.refuse
                        .contains(&post.tag)
                        .then(|| "refused".to_string())
                })
                .collect(),
            delivered: request
                .list_delivered
                .then(|| self.delivered.clone())
                .flatten(),
        })
    }
}

async fn database() -> Database {
    Database::open(Path::new(":memory:")).await.unwrap()
}

fn queued(event: &str) -> QueuedNotification {
    QueuedNotification {
        event: event.into(),
        kind: "available".into(),
        tag: task_tag("home", "T", event),
        title: format!("Available {event}"),
        body: "Write the report".into(),
        expires_at: unix_now() + 3600,
    }
}

async fn queue(database: &Database, events: &[&str]) {
    let rows: Vec<_> = events.iter().map(|event| queued(event)).collect();
    database
        .queue_notifications(&[], &rows, unix_now())
        .await
        .unwrap();
}

async fn state(database: &Database, event: &str) -> Option<OutboxState> {
    // Finished rows don't wait, so read them through a failed count or the
    // waiting list.
    let waiting = database.outbox_waiting(unix_now()).await.unwrap();
    waiting
        .into_iter()
        .find(|row| row.notification.event == event)
        .map(|row| row.state)
}

fn posted(request: &Request) -> Vec<&str> {
    request
        .posts
        .iter()
        .map(|post| post.title.as_str())
        .collect()
}

fn titles(notifications: &[QueuedNotification]) -> Vec<&str> {
    notifications
        .iter()
        .map(|notification| notification.title.as_str())
        .collect()
}

#[test]
fn tags_name_what_a_click_opens() {
    let task = Target::Task {
        workspace: "home".into(),
        task: "0123456789ABCDEF".into(),
    };
    assert_eq!(
        target(&task_tag("home", "0123456789ABCDEF", "e1")),
        Some(task.clone())
    );
    assert_eq!(target("task:home:0123456789ABCDEF"), Some(task));
    assert_eq!(
        target(&task_tag("a:b", "T", "e1")),
        Some(Target::Task {
            workspace: "a:b".into(),
            task: "T".into(),
        })
    );
    assert_eq!(
        target(&digest_tag("2026-11-06")),
        Some(Target::Due {
            date: "2026-11-06".into()
        })
    );
    assert_eq!(target(&plain_tag("available", "e1")), None);
    assert_eq!(target("launch-failed:3"), None);
    assert_eq!(target("v1:task:home:T"), None);
}

#[test]
fn events_are_unique() {
    assert_ne!(new_event(), new_event());
    assert_eq!(new_event().len(), 16);
}

#[tokio::test]
async fn waiting_rows_are_posted_once_and_marked_submitted() {
    let database = database().await;
    queue(&database, &["a", "b"]).await;
    let service = Arc::new(Service::allowing());
    let deliver: Arc<dyn Deliver> = service.clone();

    let drained = drain(&database, &deliver).await.unwrap();
    assert_eq!(titles(&drained.submitted), ["Available a", "Available b"]);
    let drained = drain(&database, &deliver).await.unwrap();
    assert!(drained.submitted.is_empty());

    let requests = service.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(posted(&requests[0]), ["Available a", "Available b"]);
    assert!(!requests[0].list_delivered);
    assert!(!database.outbox_has_work(unix_now()).await.unwrap());
}

#[tokio::test]
async fn nothing_waiting_touches_no_service() {
    let database = database().await;
    let service = Arc::new(Service::allowing());
    let deliver: Arc<dyn Deliver> = service.clone();

    drain(&database, &deliver).await.unwrap();

    assert!(service.requests().is_empty());
}

#[tokio::test]
async fn notifications_turned_off_fail_rows_with_the_reason() {
    for authorization in [Authorization::Denied, Authorization::NotAsked] {
        let database = database().await;
        queue(&database, &["a"]).await;
        let service = Arc::new(Service {
            authorization,
            ..Service::allowing()
        });
        let deliver: Arc<dyn Deliver> = service.clone();

        let drained = drain(&database, &deliver).await.unwrap();

        assert_eq!(drained.failed.len(), 1);
        assert_eq!(drained.failed[0].1, NOTIFICATIONS_OFF);
        assert_eq!(
            database
                .failed_notifications_since(NOTIFICATIONS_OFF, 0)
                .await
                .unwrap(),
            1
        );
        assert!(!database.outbox_has_work(unix_now()).await.unwrap());
    }
}

#[tokio::test]
async fn a_refused_post_fails_alone() {
    let database = database().await;
    queue(&database, &["a", "b"]).await;
    let service = Arc::new(Service {
        refuse: vec![task_tag("home", "T", "a")],
        ..Service::allowing()
    });
    let deliver: Arc<dyn Deliver> = service.clone();

    let drained = drain(&database, &deliver).await.unwrap();

    assert_eq!(titles(&drained.submitted), ["Available b"]);
    assert_eq!(drained.failed[0].0.event, "a");
    assert_eq!(
        database
            .failed_notifications_since("refused", 0)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn a_delivery_that_dies_leaves_rows_to_check() {
    let database = database().await;
    queue(&database, &["a"]).await;
    let broken: Arc<dyn Deliver> = Arc::new(Service {
        broken: true,
        ..Service::allowing()
    });

    assert!(drain(&database, &broken).await.is_err());
    assert_eq!(state(&database, "a").await, Some(OutboxState::Submitting));
}

#[tokio::test]
async fn an_unsure_row_already_shown_is_not_posted_again() {
    let database = database().await;
    queue(&database, &["shown", "lost", "new"]).await;
    database
        .set_outbox_state(
            &["shown".into(), "lost".into()],
            &OutboxState::Submitting,
            unix_now(),
        )
        .await
        .unwrap();
    let service = Arc::new(Service {
        delivered: Some(vec![task_tag("home", "T", "shown")]),
        ..Service::allowing()
    });
    let deliver: Arc<dyn Deliver> = service.clone();

    let drained = drain(&database, &deliver).await.unwrap();

    let requests = service.requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(requests[0].list_delivered && requests[0].posts.is_empty());
    assert_eq!(posted(&requests[1]), ["Available lost", "Available new"]);
    assert_eq!(
        titles(&drained.submitted),
        ["Available shown", "Available lost", "Available new"]
    );
}

#[tokio::test]
async fn a_service_that_cannot_list_gets_unsure_rows_again() {
    let database = database().await;
    queue(&database, &["a"]).await;
    database
        .set_outbox_state(&["a".into()], &OutboxState::Submitting, unix_now())
        .await
        .unwrap();
    let service = Arc::new(Service {
        delivered: None,
        ..Service::allowing()
    });
    let deliver: Arc<dyn Deliver> = service.clone();

    drain(&database, &deliver).await.unwrap();

    assert_eq!(posted(&service.requests()[1]), ["Available a"]);
}

#[tokio::test]
async fn expired_rows_are_dropped_not_posted() {
    let database = database().await;
    let mut late = queued("late");
    late.expires_at = unix_now() - 1;
    database
        .queue_notifications(&[], &[late], unix_now() - 100)
        .await
        .unwrap();
    let service = Arc::new(Service::allowing());
    let deliver: Arc<dyn Deliver> = service.clone();

    let drained = drain(&database, &deliver).await.unwrap();

    assert_eq!(drained.expired, 1);
    assert!(service.requests().is_empty());
}

#[test]
fn the_delivery_lock_is_held_by_one_process_at_a_time() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("aven.sqlite");
    assert_eq!(lock_path(&db), dir.path().join("aven.sqlite.delivery.lock"));

    let held = DeliveryLock::try_take(&db).unwrap();
    assert!(held.is_some());
    assert!(DeliveryLock::try_take(&db).unwrap().is_none());
    drop(held);
    assert!(DeliveryLock::try_take(&db).unwrap().is_some());
}

#[test]
fn the_child_protocol_round_trips() {
    let service = Service::allowing();
    let request = Request {
        posts: vec![Post {
            tag: task_tag("home", "T", "a"),
            title: "Available APP-1".into(),
            body: "Write".into(),
        }],
        list_delivered: true,
        ask: false,
    };

    let output = child::serve(&serde_json::to_vec(&request).unwrap(), &service).unwrap();
    let report: Report = serde_json::from_slice(&output).unwrap();

    assert_eq!(report.authorization, Authorization::Allowed);
    assert_eq!(report.errors, [None]);
    assert_eq!(report.delivered, Some(Vec::new()));
    assert_eq!(service.requests(), [request]);
}

#[test]
fn a_request_without_ask_does_not_ask() {
    let request: Request = serde_json::from_str(r#"{"posts":[],"list_delivered":false}"#).unwrap();
    assert!(!request.ask);
}
