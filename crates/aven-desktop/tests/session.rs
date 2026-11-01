//! Behaviour of the session over a real store on a temporary database.
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use aven_core::choices::{TaskPriority, TaskSource, TaskStatus};
use aven_core::ids::TaskId;
use aven_core::operations::TaskDraft;
use aven_core::sync::client::keys::FileProtectedStorage;
use aven_desktop::adapter::{self, Command, NewTask, Reply, ReplyBody, Request, Snapshot};
use aven_desktop::session::{Session, ToastAction};
use aven_host::config::AppConfig;
use aven_host::store::{TaskQuery, TuiStore};
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
    fn store_list(&self) -> Snapshot {
        adapter::snapshot(&self.store, Local::now())
    }

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

    let late = ReplyBody::Detail(Box::new(adapter::detail(&first_item, Local::now())));
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
    let failed = Command::Create(Box::new(NewTask::titled("New idea")));

    let applied = h.session.apply(reply(
        1,
        ReplyBody::SaveFailed(failed, "database is locked".to_string()),
    ));
    assert_eq!(applied.composer_draft.unwrap().title, "New idea");
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
    h.session.create(NewTask::titled("Fourth"));
    h.session.apply(reply(1, ReplyBody::Created(third.clone())));
    assert_eq!(h.session.selected.as_ref(), Some(&third));

    h.session.create(NewTask::titled("Fifth"));
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

/// Answers the newest write the way the actor does once the store has run it.
fn answer(h: &mut Harness, list: Snapshot) -> aven_desktop::session::Applied {
    let request = h.take_requests().iter().map(|r| r.id).max().unwrap_or(0);
    h.session
        .apply(reply(request, ReplyBody::List(Box::new(list))))
}

/// The store's list with one task changed, in place.
fn with_status(h: &Harness, id: &TaskId, status: TaskStatus) -> Snapshot {
    let mut list = adapter::snapshot(&h.store, Local::now());
    let at = list.tasks.iter().position(|row| row.id == *id).unwrap();
    list.tasks[at].status = status;
    list
}

/// The store's list once a task has left it.
fn without(h: &Harness, ids: &[&TaskId]) -> Snapshot {
    let mut list = adapter::snapshot(&h.store, Local::now());
    let tasks = std::mem::take(&mut list.tasks);
    let rows: Vec<_> = tasks
        .into_iter()
        .filter(|row| !ids.contains(&&row.id))
        .collect();
    list.groups = vec![adapter::Group {
        name: "Focus".to_string(),
        hint: String::new(),
        rows: (0..rows.len()).collect(),
    }];
    list.tasks = rows;
    list
}

fn groups(session: &Session) -> Vec<(String, Vec<String>)> {
    let snapshot = &session.snapshot;
    let titles = |rows: &[usize]| {
        rows.iter()
            .map(|task| snapshot.tasks[*task].title.clone())
            .collect()
    };
    snapshot
        .groups
        .iter()
        .map(|group| (group.name.clone(), titles(&group.rows)))
        .collect()
}

fn done_titles(session: &Session) -> Vec<String> {
    groups(session)
        .into_iter()
        .find(|(name, _)| name == "Done")
        .map_or_else(Vec::new, |(_, titles)| titles)
}

#[tokio::test]
async fn completed_tasks_gather_in_one_last_group_in_completion_order() {
    let mut h = harness(&["First", "Second", "Third"]).await;
    let (first, third) = (
        row(&h.session.snapshot, "First"),
        row(&h.session.snapshot, "Third"),
    );

    h.session.set_status(&third, TaskStatus::Done);
    let list = with_status(&h, &third, TaskStatus::Done);
    let applied = answer(&mut h, list);
    assert_eq!(applied.moved.as_ref(), Some(&third));

    h.session.set_status(&first, TaskStatus::Done);
    let mut list = without(&h, &[&third]);
    let at = list.tasks.iter().position(|row| row.id == first).unwrap();
    list.tasks[at].status = TaskStatus::Done;
    answer(&mut h, list);

    let shown = groups(&h.session);
    assert_eq!(shown.last().unwrap().0, "Done");
    assert_eq!(shown.iter().filter(|(name, _)| name == "Done").count(), 1);
    assert_eq!(done_titles(&h.session), ["Third", "First"]);
    assert!(shown[..shown.len() - 1].iter().all(|(_, titles)| {
        !titles.contains(&"First".to_string()) && !titles.contains(&"Third".to_string())
    }));
}

#[tokio::test]
async fn an_outside_refresh_keeps_the_done_group_and_plays_no_motion() {
    let mut h = harness(&["First", "Second"]).await;
    let second = row(&h.session.snapshot, "Second");
    h.session.set_status(&second, TaskStatus::Done);
    let list = with_status(&h, &second, TaskStatus::Done);
    answer(&mut h, list);

    let list = without(&h, &[&second]);
    let applied = h.session.apply(reply(100, ReplyBody::List(Box::new(list))));

    assert_eq!(applied.moved, None);
    assert_eq!(done_titles(&h.session), ["Second"]);
}

#[tokio::test]
async fn the_users_refresh_clears_the_done_group_without_motion() {
    let mut h = harness(&["First", "Second"]).await;
    let second = row(&h.session.snapshot, "Second");
    h.session.select(second.clone());
    h.session.set_status(&second, TaskStatus::Done);
    let list = with_status(&h, &second, TaskStatus::Done);
    answer(&mut h, list);
    h.take_requests();

    h.session.refresh_view();
    let sent = h.take_requests();
    assert!(matches!(sent[0].command, Command::RefreshView));

    // A list from before the refresh must not bring the row back.
    let stale = with_status(&h, &second, TaskStatus::Done);
    h.session
        .apply(reply(sent[0].id - 1, ReplyBody::List(Box::new(stale))));
    assert_eq!(done_titles(&h.session), ["Second"]);

    let list = without(&h, &[&second]);
    let applied = h
        .session
        .apply(reply(sent[0].id, ReplyBody::List(Box::new(list))));
    assert_eq!(applied.moved, None);
    assert!(done_titles(&h.session).is_empty());
    assert!(h.session.snapshot.row(&second).is_none());
}

#[tokio::test]
async fn a_reopened_task_leaves_the_done_group() {
    let mut h = harness(&["First", "Second"]).await;
    let second = row(&h.session.snapshot, "Second");
    h.session.set_status(&second, TaskStatus::Done);
    let list = with_status(&h, &second, TaskStatus::Done);
    answer(&mut h, list);

    let list = adapter::snapshot(&h.store, Local::now());
    h.session.apply(reply(100, ReplyBody::List(Box::new(list))));

    assert!(done_titles(&h.session).is_empty());
    assert!(h.session.snapshot.row(&second).is_some());
}

#[tokio::test]
async fn completing_the_selection_moves_it_to_the_row_in_its_place() {
    let mut h = harness(&["First", "Second", "Third"]).await;
    let order: Vec<_> = h
        .session
        .snapshot
        .ordered()
        .map(|task| h.session.snapshot.tasks[task].id.clone())
        .collect();
    h.session.select(order[1].clone());

    h.session.set_status(&order[1], TaskStatus::Done);
    let list = with_status(&h, &order[1], TaskStatus::Done);
    let applied = answer(&mut h, list);
    assert!(applied.selection);
    assert_eq!(h.session.selected.as_ref(), Some(&order[2]));

    // The last open row falls back, like the TUI, and never enters the group.
    h.session.set_status(&order[2], TaskStatus::Done);
    let mut list = with_status(&h, &order[2], TaskStatus::Done);
    let at = list
        .tasks
        .iter()
        .position(|row| row.id == order[1])
        .unwrap();
    list.tasks[at].status = TaskStatus::Done;
    answer(&mut h, list);
    assert_eq!(h.session.selected.as_ref(), Some(&order[0]));

    // With nothing open left, the selection stays on its row.
    h.session.set_status(&order[0], TaskStatus::Done);
    let list = h.session.snapshot.clone();
    answer(&mut h, list);
    assert_eq!(h.session.selected.as_ref(), Some(&order[0]));
}

#[tokio::test]
async fn completing_another_row_and_outside_changes_leave_the_selection() {
    let mut h = harness(&["First", "Second", "Third"]).await;
    let (first, second) = (
        row(&h.session.snapshot, "First"),
        row(&h.session.snapshot, "Second"),
    );
    h.session.select(first.clone());
    h.session.set_status(&second, TaskStatus::Done);
    let list = with_status(&h, &second, TaskStatus::Done);
    answer(&mut h, list);
    assert_eq!(h.session.selected.as_ref(), Some(&first));

    let mut list = with_status(&h, &first, TaskStatus::Done);
    let at = list.tasks.iter().position(|row| row.id == second).unwrap();
    list.tasks[at].status = TaskStatus::Done;
    let applied = h.session.apply(reply(100, ReplyBody::List(Box::new(list))));
    assert_eq!(h.session.selected.as_ref(), Some(&first));
    assert_eq!(applied.moved, None);
    assert_eq!(done_titles(&h.session).len(), 2);
}

#[tokio::test]
async fn a_priority_change_reports_its_move_once_and_keeps_the_selection() {
    let mut h = harness(&["First", "Second"]).await;
    let first = row(&h.session.snapshot, "First");
    h.session.select(first.clone());
    h.session.set_priority(&first, TaskPriority::High);
    let list = adapter::snapshot(&h.store, Local::now());
    let applied = answer(&mut h, list);
    assert_eq!(applied.moved.as_ref(), Some(&first));
    assert_eq!(h.session.selected.as_ref(), Some(&first));

    let list = adapter::snapshot(&h.store, Local::now());
    let applied = h.session.apply(reply(100, ReplyBody::List(Box::new(list))));
    assert_eq!(applied.moved, None);
}

async fn until_done_group(session: &mut Session, replies: &mut adapter::Replies, shown: bool) {
    loop {
        let reply = tokio::time::timeout(Duration::from_secs(5), replies.recv())
            .await
            .unwrap()
            .unwrap();
        let list = matches!(reply.body, ReplyBody::List(_));
        session.apply(reply);
        if list && done_titles(session).is_empty() != shown {
            return;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_row_the_store_dropped_can_be_reopened_from_the_done_group() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("db.sqlite");
    let store = store_with(&path, &["First", "Second"]).await;
    drop(store);
    let storage = Arc::new(FileProtectedStorage::new(directory.path().join("keys")));
    let launch = adapter::Launch {
        database: path,
        config: AppConfig::default(),
        storage,
    };
    let (mut session, mut replies) =
        adapter::start(tokio::runtime::Handle::current(), launch).unwrap();
    let first = row(&session.snapshot, "First");
    let second = row(&session.snapshot, "Second");
    session.select(second.clone());

    session.set_status(&second, TaskStatus::Done);
    until_done_group(&mut session, &mut replies, true).await;
    assert_eq!(session.selected.as_ref(), Some(&first));

    // The next refresh drops the row from the store. The view keeps it.
    session.select(first.clone());
    session.apply(reply(0, ReplyBody::RefreshFailed("x".to_string())));
    session.run_toast_action();
    tokio::time::sleep(Duration::from_millis(200)).await;
    while let Ok(reply) = replies.try_recv() {
        session.apply(reply);
    }
    assert_eq!(done_titles(&session), ["Second"]);

    session.set_status(&second, TaskStatus::Todo);
    until_done_group(&mut session, &mut replies, false).await;
    assert!(session.toast.unwrap().message.ends_with("moved to Todo"));
    assert_eq!(
        session.snapshot.row(&second).unwrap().status,
        TaskStatus::Todo
    );
}

#[tokio::test]
async fn canceled_rows_join_the_same_end_group_as_done_rows() {
    let mut h = harness(&["First", "Second", "Third"]).await;
    let first = row(&h.session.snapshot, "First");
    let second = row(&h.session.snapshot, "Second");
    h.session.set_status(&first, TaskStatus::Done);
    h.session.set_status(&second, TaskStatus::Canceled);
    let mut list = with_status(&h, &first, TaskStatus::Done);
    let at = list.tasks.iter().position(|task| task.id == second);
    list.tasks[at.unwrap()].status = TaskStatus::Canceled;
    answer(&mut h, list);

    assert_eq!(done_titles(&h.session), ["First", "Second"]);
    assert_eq!(groups(&h.session).len(), 2);
}

#[tokio::test]
async fn switching_views_clears_the_done_group_and_the_selection() {
    let mut h = harness(&["First", "Second"]).await;
    let first = row(&h.session.snapshot, "First");
    h.session.set_status(&first, TaskStatus::Done);
    let list = with_status(&h, &first, TaskStatus::Done);
    answer(&mut h, list);
    assert_eq!(done_titles(&h.session), ["First"]);

    h.take_requests();
    h.session.switch_view(Command::ShowView(TaskQuery::Inbox));
    assert_eq!(h.session.selected, None);
    let commands: Vec<_> = h.take_requests().into_iter().map(|r| r.command).collect();
    assert_eq!(
        commands,
        [Command::RefreshView, Command::ShowView(TaskQuery::Inbox)]
    );
    let list = h.store_list();
    h.session
        .apply(reply(u64::MAX, ReplyBody::List(Box::new(list))));
    assert_eq!(done_titles(&h.session), Vec::<String>::new());
    assert!(h.session.selected.is_some());
}

#[tokio::test]
async fn a_view_of_closed_rows_keeps_them_in_their_own_groups() {
    let mut h = harness(&["First"]).await;
    let first = row(&h.session.snapshot, "First");
    let mut list = with_status(&h, &first, TaskStatus::Done);
    list.view.query = TaskQuery::Done;
    answer(&mut h, list);
    assert_eq!(done_titles(&h.session), Vec::<String>::new());
    assert_eq!(h.session.snapshot.ordered().count(), 1);
}

#[tokio::test]
async fn a_peeked_task_takes_actions_and_steps_back_along_the_trail() {
    let mut h = harness(&["First", "Second"]).await;
    let first = row(&h.session.snapshot, "First");
    h.session.select(first.clone());
    let outside = TaskId::new();
    h.session.follow(outside.clone());
    assert_eq!(h.session.target(), Some(&outside));
    assert_eq!(h.session.selected.as_ref(), Some(&first));
    assert_eq!(h.session.trail.len(), 1);
    h.session.trail_back();
    assert_eq!(h.session.target(), Some(&first));
    assert!(h.session.trail.is_empty());
}

#[tokio::test]
async fn a_saved_write_offers_undo_and_its_way_there() {
    let mut h = harness(&["First"]).await;
    let go = adapter::Go {
        label: "View in Upcoming",
        command: Command::ShowView(TaskQuery::Upcoming),
    };
    h.session.apply(reply(
        0,
        ReplyBody::Saved {
            message: "Deferred".to_string(),
            go: Some(go.clone()),
            undo: true,
        },
    ));
    let toast = h.session.toast.clone().unwrap();
    assert!(toast.undo);
    assert_eq!(toast.action, Some(ToastAction::Go(go)));
    h.take_requests();
    h.session.run_toast_undo();
    let commands: Vec<_> = h.take_requests().into_iter().map(|r| r.command).collect();
    assert_eq!(commands, [Command::Undo]);
}
