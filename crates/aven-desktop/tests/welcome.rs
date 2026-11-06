//! The first-launch welcome over a real store on a temporary database.
#![cfg(target_os = "macos")]
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use aven_core::choices::TaskSource;
use aven_core::db::Database;
use aven_core::local_state::OnboardingStatus;
use aven_core::operations::TaskDraft;
use aven_core::sync::client::keys::FileProtectedStorage;
use aven_desktop::adapter::{self, NewTask, Replies, ReplyBody};
use aven_desktop::session::Session;
use aven_host::config::AppConfig;
use tempfile::TempDir;

fn start(directory: &TempDir) -> (Session, Replies) {
    let launch = adapter::Launch {
        database: directory.path().join("db.sqlite"),
        config: aven_host::test_support::isolated_config(),
        config_file: None,
        storage: Some(Arc::new(FileProtectedStorage::new(
            directory.path().join("keys"),
        ))),
        version: "0.0.0".to_string(),
    };
    adapter::start(tokio::runtime::Handle::current(), launch).unwrap()
}

async fn database(directory: &TempDir) -> Database {
    Database::open(&directory.path().join("db.sqlite"))
        .await
        .unwrap()
}

async fn with_task(path: &Path) {
    let mut store = adapter::open_store(path, &AppConfig::default())
        .await
        .unwrap();
    let draft = TaskDraft {
        metadata: Vec::new(),
        title: "Stored".to_string(),
        description: String::new(),
        project: Some("work".to_string()),
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

/// Applies replies until a list holds a row with this title.
async fn until_listed(session: &mut Session, replies: &mut Replies, title: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let reply = replies.recv().await.unwrap();
            let list = matches!(reply.body, ReplyBody::List(_));
            session.apply(reply);
            if list && session.snapshot.tasks.iter().any(|row| row.title == title) {
                return;
            }
        }
    })
    .await
    .unwrap();
}

/// Applies replies until the store has answered every request sent so far.
async fn until_settled(session: &mut Session, replies: &mut Replies) {
    while let Ok(Some(reply)) =
        tokio::time::timeout(Duration::from_millis(500), replies.recv()).await
    {
        session.apply(reply);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_welcome_shows_only_on_a_fresh_database_never_welcomed() {
    let fresh = tempfile::tempdir().unwrap();
    let (session, _replies) = start(&fresh);
    assert!(session.snapshot.welcome);

    let welcomed = tempfile::tempdir().unwrap();
    database(&welcomed)
        .await
        .complete_onboarding()
        .await
        .unwrap();
    let (session, _replies) = start(&welcomed);
    assert!(!session.snapshot.welcome);

    let established = tempfile::tempdir().unwrap();
    with_task(&established.path().join("db.sqlite")).await;
    let (session, _replies) = start(&established);
    assert!(!session.snapshot.welcome);
    // Like the terminal app, a database with tasks is marked as welcomed.
    assert_eq!(
        database(&established)
            .await
            .onboarding_status()
            .await
            .unwrap(),
        OnboardingStatus::Complete
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_first_task_creates_the_first_project_and_ends_the_welcome() {
    let directory = tempfile::tempdir().unwrap();
    let (mut session, mut replies) = start(&directory);
    assert!(session.snapshot.projects.is_empty());
    assert_eq!(session.first_project_name(), "Personal");
    assert_eq!(adapter::first_project_prefix("Personal"), "PRS");

    session.create_first_task(NewTask::titled("Water the plants"));
    assert!(!session.snapshot.welcome);
    until_listed(&mut session, &mut replies, "Water the plants").await;
    until_settled(&mut session, &mut replies).await;

    let projects: Vec<_> = (session.snapshot.projects.iter())
        .map(|project| (project.name.as_str(), project.prefix.as_str()))
        .collect();
    assert_eq!(projects, [("Personal", "PRS")]);
    let row = &session.snapshot.tasks[0];
    assert!(row.reference.starts_with("PRS-"), "{}", row.reference);
    assert_eq!(session.selected.as_ref(), Some(&row.id));
    assert!(!session.snapshot.welcome);
    assert_eq!(
        database(&directory)
            .await
            .onboarding_status()
            .await
            .unwrap(),
        OnboardingStatus::Complete
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_given_before_the_first_task_names_the_project_and_its_prefix() {
    let directory = tempfile::tempdir().unwrap();
    let (mut session, mut replies) = start(&directory);
    session.name_first_project("Home Lab".to_string());
    assert_eq!(adapter::first_project_prefix("Home Lab"), "HL");

    session.create_first_task(NewTask::titled("Flash the router"));
    until_listed(&mut session, &mut replies, "Flash the router").await;
    until_settled(&mut session, &mut replies).await;

    let project = &session.snapshot.projects[0];
    assert_eq!(
        (
            project.name.as_str(),
            project.key.as_str(),
            project.prefix.as_str()
        ),
        ("Home Lab", "home-lab", "HL")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_composer_on_an_empty_workspace_creates_the_first_project_too() {
    let directory = tempfile::tempdir().unwrap();
    let (mut session, mut replies) = start(&directory);

    session.create(NewTask::titled("From the composer"));
    until_listed(&mut session, &mut replies, "From the composer").await;

    let row = &session.snapshot.tasks[0];
    assert_eq!(row.project_key, "personal");
}

#[tokio::test(flavor = "multi_thread")]
async fn skipping_the_welcome_marks_it_done_for_good() {
    let directory = tempfile::tempdir().unwrap();
    let (mut session, mut replies) = start(&directory);
    session.skip_welcome();
    assert!(!session.snapshot.welcome);
    until_settled(&mut session, &mut replies).await;
    assert!(!session.snapshot.welcome);
    assert!(session.snapshot.projects.is_empty());

    assert_eq!(
        database(&directory)
            .await
            .onboarding_status()
            .await
            .unwrap(),
        OnboardingStatus::Complete
    );
    let (session, _replies) = start(&directory);
    assert!(!session.snapshot.welcome);
}
