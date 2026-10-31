//! Behaviour of the session over a real store on a temporary database.
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use aven_core::choices::{TaskPriority, TaskSource, TaskStatus};
use aven_core::ids::TaskId;
use aven_core::operations::TaskDraft;
use aven_core::sync::client::keys::FileProtectedStorage;
use aven_desktop::adapter::{self, Command, Reply, ReplyBody, Request, Snapshot};
use aven_desktop::session::{Session, ToastAction};
use aven_host::config::AppConfig;
use aven_host::store::TuiStore;
use chrono::Local;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

fn row(snapshot: &Snapshot, title: &str) -> TaskId {
    let row = snapshot.tasks.iter().find(|row| row.title == title);
    row.expect("a row with that title").id.clone()
}

struct Harness {
    _directory: tempfile::TempDir,
    store: TuiStore,
    requests: UnboundedReceiver<Request>,
    session: Session,
}

fn reply(request: u64, body: ReplyBody) -> Reply {
    Reply { request, body }
}

async fn store_with(path: &Path, titles: &[&str]) -> TuiStore {
    let mut store = adapter::open_store(path, &AppConfig::default())
        .await
        .unwrap();
    for title in titles {
        let draft = TaskDraft {
            metadata: Vec::new(),
            title: title.to_string(),
            description: String::new(),
            project: None,
            status: Some("todo".to_string()),
            priority: "none".to_string(),
            source: TaskSource::Unknown,
            labels: Vec::new(),
            available_at: None,
            due_on: None,
            is_epic: false,
        };
        store.create_task(draft, None).await.unwrap();
    }
    store
}

async fn harness(titles: &[&str]) -> Harness {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("db.sqlite");
    let store = store_with(&path, titles).await;
    let (requests_tx, requests) = unbounded_channel();
    let first = adapter::snapshot(&store, Local::now());
    let session = Session::new(first, requests_tx);
    Harness {
        _directory: directory,
        store,
        requests,
        session,
    }
}

impl Harness {
    fn take_requests(&mut self) -> Vec<Request> {
        let mut taken = Vec::new();
        while let Ok(request) = self.requests.try_recv() {
            taken.push(request);
        }
        taken
    }

    fn list(&self, handled: u64) -> Reply {
        let list = adapter::snapshot(&self.store, Local::now());
        reply(handled, ReplyBody::List(Box::new(list)))
    }
}

#[tokio::test]
async fn refresh_keeps_selection_and_moves_to_the_neighbour_when_it_goes() {
    let mut h = harness(&["First", "Second", "Third"]).await;
    let second = row(&h.session.snapshot, "Second");
    h.session.select(second.clone());
    h.session
        .note_drafts
        .insert(second.clone(), "half a thought".to_string());

    let list = h.list(10);
    let applied = h.session.apply(list);
    assert!(!applied.selection);
    assert_eq!(h.session.selected.as_ref(), Some(&second));
    assert_eq!(h.session.note_drafts[&second], "half a thought");

    let mut without = adapter::snapshot(&h.store, Local::now());
    let ordered: Vec<_> = without
        .ordered()
        .map(|ix| without.tasks[ix].id.clone())
        .collect();
    let at = ordered.iter().position(|id| *id == second).unwrap();
    let vanished = without
        .tasks
        .iter()
        .position(|row| row.id == second)
        .unwrap();
    for group in &mut without.groups {
        group.rows.retain(|ix| *ix != vanished);
    }
    without.tasks[vanished].id = TaskId::new();
    let applied = h
        .session
        .apply(reply(11, ReplyBody::List(Box::new(without))));
    assert!(applied.selection);
    let remaining: Vec<_> = ordered.iter().filter(|id| **id != second).collect();
    assert_eq!(
        h.session.selected.as_ref(),
        Some(remaining[at.min(remaining.len() - 1)])
    );
}

#[tokio::test]
async fn a_list_older_than_the_last_write_is_dropped() {
    let mut h = harness(&["Only"]).await;
    let only = row(&h.session.snapshot, "Only");
    let stale = h.list(0);
    h.session.set_priority(&only, TaskPriority::Urgent);

    assert!(!h.session.apply(stale).list);
    let shown = h.session.snapshot.row(&only).unwrap();
    assert_eq!(
        shown.priority,
        TaskPriority::Urgent,
        "the optimistic change stays"
    );
}

#[tokio::test]
async fn a_detail_for_an_earlier_selection_is_dropped() {
    let mut h = harness(&["First", "Second"]).await;
    let (first, second) = (
        row(&h.session.snapshot, "First"),
        row(&h.session.snapshot, "Second"),
    );
    h.session.select(first.clone());
    let first_item = h.store.load_task_item(&first).await.unwrap().unwrap();
    let requested = h.take_requests().last().map_or(0, |request| request.id);
    h.session.select(second.clone());

    let late = ReplyBody::Detail(Box::new(adapter::detail(&first_item)));
    h.session.apply(reply(requested, late));
    assert_eq!(h.session.detail, None);
}

#[tokio::test]
async fn a_failed_save_offers_retry_and_a_failed_refresh_never_repeats_the_write() {
    let mut h = harness(&["Only"]).await;
    let only = row(&h.session.snapshot, "Only");
    let command = Command::SetPriority(only.clone(), TaskPriority::High);

    h.session.apply(reply(
        1,
        ReplyBody::SaveFailed(command.clone(), "database is locked".to_string()),
    ));
    let toast = h.session.toast.clone().unwrap();
    assert_eq!(toast.action, Some(ToastAction::Retry(command.clone())));
    h.take_requests();
    h.session.run_toast_action();
    let sent: Vec<_> = h
        .take_requests()
        .into_iter()
        .map(|request| request.command)
        .collect();
    assert_eq!(sent, vec![command]);

    h.session
        .apply(reply(2, ReplyBody::SavedRefreshFailed("gone".to_string())));
    assert_eq!(
        h.session.toast.clone().unwrap().action,
        Some(ToastAction::Refresh)
    );
    h.session.run_toast_action();
    let sent = h.take_requests();
    assert!(sent.iter().all(|request| !request.command.is_mutation()));
    assert!(
        sent.iter()
            .any(|request| request.command == Command::Refresh)
    );
}

#[tokio::test]
async fn a_failed_note_returns_to_its_draft() {
    let mut h = harness(&["Only"]).await;
    let only = row(&h.session.snapshot, "Only");
    h.session.add_note(&only, "keep me".to_string());
    let failed = Command::AddNote(only.clone(), "keep me".to_string());

    let applied = h.session.apply(reply(
        1,
        ReplyBody::SaveFailed(failed, "database is locked".to_string()),
    ));
    assert!(applied.draft);
    assert_eq!(h.session.note_drafts[&only], "keep me");
    assert_eq!(
        h.session.toast.clone().unwrap().action,
        None,
        "Enter saves the draft again"
    );
}

#[tokio::test]
async fn a_note_typed_while_the_save_failed_follows_the_returned_text() {
    let mut h = harness(&["Only"]).await;
    let only = row(&h.session.snapshot, "Only");
    h.session.add_note(&only, "first".to_string());
    h.session
        .note_drafts
        .insert(only.clone(), "second".to_string());

    let failed = Command::AddNote(only.clone(), "first".to_string());
    h.session.apply(reply(
        1,
        ReplyBody::SaveFailed(failed, "database is locked".to_string()),
    ));
    let draft = &h.session.note_drafts[&only];
    assert!(draft.starts_with("first") && draft.ends_with("second"));
}

#[tokio::test]
async fn a_failed_create_returns_its_title_to_the_composer() {
    let mut h = harness(&["Only"]).await;
    let failed = Command::Create("New idea".to_string());

    let applied = h.session.apply(reply(
        1,
        ReplyBody::SaveFailed(failed, "database is locked".to_string()),
    ));
    assert_eq!(applied.composer_draft.as_deref(), Some("New idea"));
    assert_eq!(
        h.session.toast.clone().unwrap().action,
        None,
        "the composer saves it again, so no retry can duplicate it"
    );
}

#[tokio::test]
async fn a_created_task_takes_selection_only_if_it_has_not_moved() {
    let mut h = harness(&["First", "Second", "Third"]).await;
    let (first, second, third) = (
        row(&h.session.snapshot, "First"),
        row(&h.session.snapshot, "Second"),
        row(&h.session.snapshot, "Third"),
    );
    h.session.select(first.clone());
    h.session.create("Fourth".to_string());
    h.session.apply(reply(1, ReplyBody::Created(third.clone())));
    assert_eq!(h.session.selected.as_ref(), Some(&third));

    h.session.create("Fifth".to_string());
    h.session.select(second.clone());
    h.session.apply(reply(2, ReplyBody::Created(first)));
    assert_eq!(h.session.selected.as_ref(), Some(&second));
}

#[tokio::test]
async fn motion_starts_only_from_local_action_and_not_under_reduced_motion() {
    let mut h = harness(&["First", "Second"]).await;
    let (first, second) = (
        row(&h.session.snapshot, "First"),
        row(&h.session.snapshot, "Second"),
    );

    let mut outside = adapter::snapshot(&h.store, Local::now());
    let changed = outside
        .tasks
        .iter()
        .position(|row| row.id == second)
        .unwrap();
    outside.tasks[changed].status = TaskStatus::Done;
    outside.tasks[changed].priority = TaskPriority::Urgent;
    h.session
        .apply(reply(0, ReplyBody::List(Box::new(outside))));
    assert_eq!(h.session.motion.completion(&second), None);
    assert_eq!(h.session.motion.priority_change(&second), None);

    h.session.set_status(&first, TaskStatus::Done);
    assert!(h.session.motion.completion(&first).is_some());

    h.session.motion.set_reduced(true);
    h.session.set_status(&first, TaskStatus::Todo);
    h.session.set_status(&first, TaskStatus::Done);
    h.session.set_priority(&first, TaskPriority::High);
    assert_eq!(h.session.motion.completion(&first), None);
    assert_eq!(h.session.motion.priority_change(&first), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_run_in_order_and_outside_writes_arrive() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("db.sqlite");
    let mut outside = store_with(&path, &["Stored"]).await;

    let launch = adapter::Launch {
        database: path.clone(),
        config: AppConfig::default(),
        storage: Arc::new(FileProtectedStorage::new(directory.path().join("keys"))),
    };
    let (mut session, mut replies) =
        adapter::start(tokio::runtime::Handle::current(), launch).unwrap();
    let stored = row(&session.snapshot, "Stored");

    session.set_title(&stored, "Renamed".to_string());
    session.set_title(&stored, "Renamed twice".to_string());
    outside.refresh(None).await.unwrap();
    let draft = TaskDraft {
        metadata: Vec::new(),
        title: "From elsewhere".to_string(),
        description: String::new(),
        project: None,
        status: Some("todo".to_string()),
        priority: "none".to_string(),
        source: TaskSource::Unknown,
        labels: Vec::new(),
        available_at: None,
        due_on: None,
        is_epic: false,
    };
    outside.create_task(draft, None).await.unwrap();

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            session.apply(replies.recv().await.unwrap());
            let saved = session.snapshot.row(&stored).unwrap().title == "Renamed twice";
            let arrived = session
                .snapshot
                .tasks
                .iter()
                .any(|row| row.title == "From elsewhere");
            if saved && arrived {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(session.selected.as_ref(), Some(&stored));

    let reread = adapter::open_store(&path, &AppConfig::default())
        .await
        .unwrap();
    assert_eq!(reread.tasks.len(), 2);
}
