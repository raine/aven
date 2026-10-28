use std::path::Path;

use aven_core::api::Store;
use aven_core::choices::TaskSource;
use aven_core::db::Database;
use aven_core::operations::{MoveTasksInput, TaskDraft, TaskUpdate};
use aven_core::test_support::encrypted_sync::EncryptedSyncServer;

fn draft() -> TaskDraft {
    TaskDraft {
        title: "Original title".to_string(),
        description: String::new(),
        project: Some("Source".to_string()),
        status: Some("todo".to_string()),
        priority: "none".to_string(),
        source: TaskSource::Cli,
        labels: Vec::new(),
        metadata: Vec::new(),
        available_at: None,
        due_on: None,
        is_epic: false,
    }
}

#[tokio::test]
async fn move_and_late_source_edit_converge_across_replicas() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first.sqlite");
    let second = directory.path().join("second.sqlite");
    let server = EncryptedSyncServer::new().await;
    let first_db = Database::open(&first).await.unwrap();
    let source = first_db.list_workspaces().await.unwrap().remove(0);
    let target = first_db.create_workspace("Target").await.unwrap();
    first_db
        .create_project(&source, "Source", None)
        .await
        .unwrap();
    first_db
        .create_project(&source, "Alternate", None)
        .await
        .unwrap();
    first_db
        .create_project(&target, "Target", None)
        .await
        .unwrap();
    let task = first_db.create_task(&source, draft()).await.unwrap().task;
    let mut peer_draft = draft();
    peer_draft.title = "Source peer".to_string();
    let peer = first_db
        .create_task(&source, peer_draft)
        .await
        .unwrap()
        .task;
    drop(first_db);
    aven_core::db::backup_database(&first, &second)
        .await
        .unwrap();
    settle(&first, &second, &server).await;

    let second_db = Database::open(&second).await.unwrap();
    let second_source = second_db.workspace_for_id(&source.id).await.unwrap();
    second_db
        .add_task_dependency(&second_source, &task.id, &peer.id)
        .await
        .unwrap();
    second_db
        .update_task(
            &second_source,
            &task.id,
            TaskUpdate {
                set_metadata: vec![aven_core::metadata::TaskMetadataInput {
                    expected_field_id: None,
                    key: "late-key".into(),
                    value: "late value".into(),
                }],
                title: Some("Edited while offline".to_string()),
                project: Some("Alternate".to_string()),
                ..TaskUpdate::default()
            },
        )
        .await
        .unwrap();
    drop(second_db);

    let first_db = Database::open(&first).await.unwrap();
    first_db
        .move_tasks(
            &source,
            MoveTasksInput {
                task_ids: vec![task.id.clone()],
                target_workspace: target.clone(),
                target_project: "Target".to_string(),
            },
        )
        .await
        .unwrap();
    drop(first_db);

    drain(&first, &server).await;
    drain(&second, &server).await;
    drain(&first, &server).await;

    for path in [&first, &second] {
        let store = Store::open(path).await.unwrap();
        let detail = store.task_detail(&target.id, &task.id).await.unwrap();
        assert_eq!(detail.workspace_id, target.id, "{}", path.display());
        assert_eq!(detail.project_key, "target", "{}", path.display());
        assert_eq!(detail.title, "Edited while offline", "{}", path.display());
        assert!(detail.blocked_by.is_empty(), "{}", path.display());
        drop(store);
        let database = Database::open(path).await.unwrap();
        assert_eq!(
            database.task_metadata(&target.id, &task.id).await.unwrap()[0].value,
            "late value"
        );
        let facts = database.sync_persistence_status().await.unwrap();
        assert_eq!(facts.pending_changes, 0, "{}: {facts:?}", path.display());
    }
}

async fn settle(first: &Path, second: &Path, server: &EncryptedSyncServer) {
    drain(first, server).await;
    drain(second, server).await;
    drain(first, server).await;
}

async fn drain(path: &Path, server: &EncryptedSyncServer) {
    let database = Database::open(path).await.unwrap();
    server.sync(&database).await.unwrap();
}

#[tokio::test]
async fn metadata_edits_after_a_pending_move_survive_acknowledgement_and_reopen() {
    use aven_core::metadata::TaskMetadataInput;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("first.sqlite");
    let peer_path = dir.path().join("peer.sqlite");
    let server = EncryptedSyncServer::new().await;
    let db = Database::open(&path).await.unwrap();
    let source = db.list_workspaces().await.unwrap().remove(0);
    db.create_project(&source, "Source", None).await.unwrap();
    let target = db.create_workspace("Target").await.unwrap();
    db.create_project(&target, "Target", None).await.unwrap();
    let input = |value: &str| TaskMetadataInput {
        expected_field_id: None,
        key: "owner".into(),
        value: value.into(),
    };
    let mut d = draft();
    d.metadata = vec![input("A")];
    let task = db.create_task(&source, d).await.unwrap().task;
    server.sync(&db).await.unwrap();
    let peer = Database::open(&peer_path).await.unwrap();
    server.sync(&peer).await.unwrap();
    db.move_tasks(
        &source,
        MoveTasksInput {
            task_ids: vec![task.id.clone()],
            target_workspace: target.clone(),
            target_project: "Target".into(),
        },
    )
    .await
    .unwrap();
    for value in ["B", "C"] {
        db.update_task(
            &target,
            &task.id,
            TaskUpdate {
                set_metadata: vec![input(value)],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    drop(db);
    drain(&path, &server).await;
    server.sync(&peer).await.unwrap();
    for path in [&path, &peer_path] {
        let db = Database::open(path).await.unwrap();
        assert_eq!(
            db.task_metadata(&target.id, &task.id).await.unwrap()[0].value,
            "C"
        );
        assert_eq!(db.sync_persistence_status().await.unwrap().conflicts, 0);
    }
}

#[tokio::test]
async fn moved_and_unmoved_dependency_edges_keep_one_cycle_resolution_order() {
    let dir = tempfile::tempdir().unwrap();
    let first_path = dir.path().join("first.sqlite");
    let second_path = dir.path().join("second.sqlite");
    let server = EncryptedSyncServer::new().await;
    let first = Database::open(&first_path).await.unwrap();
    let second = Database::open(&second_path).await.unwrap();
    let source = first.list_workspaces().await.unwrap().remove(0);
    first.create_project(&source, "Source", None).await.unwrap();
    let target = first.create_workspace("Target").await.unwrap();
    first.create_project(&target, "Target", None).await.unwrap();
    let x = first.create_task(&source, draft()).await.unwrap().task;
    let mut d = draft();
    d.project = Some("Target".into());
    let y = first.create_task(&target, d).await.unwrap().task;
    let mut d = draft();
    d.project = Some("Target".into());
    let z = first.create_task(&target, d).await.unwrap().task;
    server.sync(&first).await.unwrap();
    server.sync(&second).await.unwrap();
    first
        .move_tasks(
            &source,
            MoveTasksInput {
                task_ids: vec![x.id.clone()],
                target_workspace: target.clone(),
                target_project: "Target".into(),
            },
        )
        .await
        .unwrap();
    server.sync(&first).await.unwrap();
    server.sync(&second).await.unwrap();
    first
        .add_task_dependency(&target, &x.id, &y.id)
        .await
        .unwrap();
    second
        .add_task_dependency(&target, &z.id, &x.id)
        .await
        .unwrap();
    second
        .add_task_dependency(&target, &y.id, &z.id)
        .await
        .unwrap();
    server.sync(&first).await.unwrap();
    server.sync(&second).await.unwrap();
    server.sync(&first).await.unwrap();
    for db in [&first, &second] {
        let export = db.export_data("now".into()).await.unwrap();
        let edges = export.tables.task_dependencies;
        assert_eq!(edges.len(), 2);
        assert!(
            edges
                .iter()
                .any(|e| e.task_id == x.id && e.depends_on_task_id == y.id)
        );
        assert!(
            edges
                .iter()
                .any(|e| e.task_id == z.id && e.depends_on_task_id == x.id)
        );
    }
}

#[tokio::test]
async fn move_does_not_reapply_label_deletion_to_an_unmoved_task() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("first.sqlite"))
        .await
        .unwrap();
    let peer = Database::open(&dir.path().join("peer.sqlite"))
        .await
        .unwrap();
    let server = EncryptedSyncServer::new().await;
    let source = db.list_workspaces().await.unwrap().remove(0);
    db.create_project(&source, "Source", None).await.unwrap();
    let target = db.create_workspace("Target").await.unwrap();
    db.create_project(&target, "Target", None).await.unwrap();
    db.create_label(&source, "tag").await.unwrap();
    let moved = db.create_task(&source, draft()).await.unwrap().task;
    let untouched = db.create_task(&source, draft()).await.unwrap().task;
    server.sync(&db).await.unwrap();
    server.sync(&peer).await.unwrap();
    db.move_tasks(
        &source,
        MoveTasksInput {
            task_ids: vec![moved.id],
            target_workspace: target,
            target_project: "Target".into(),
        },
    )
    .await
    .unwrap();
    db.delete_label(&source, "tag").await.unwrap();
    db.update_task(
        &source,
        &untouched.id,
        TaskUpdate {
            add_labels: vec!["tag".into()],
            create_missing_labels: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    server.sync(&db).await.unwrap();
    server.sync(&peer).await.unwrap();
    for db in [&db, &peer] {
        assert_eq!(
            db.task_labels(&source.id, &untouched.id).await.unwrap(),
            ["tag"]
        );
    }
}

#[tokio::test]
async fn move_acknowledgement_preserves_later_undo_and_epic_promotion() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("first.sqlite"))
        .await
        .unwrap();
    let peer = Database::open(&dir.path().join("peer.sqlite"))
        .await
        .unwrap();
    let server = EncryptedSyncServer::new().await;
    let source = db.list_workspaces().await.unwrap().remove(0);
    db.create_project(&source, "Source", None).await.unwrap();
    let target = db.create_workspace("Target").await.unwrap();
    db.create_project(&target, "Target", None).await.unwrap();
    let parent = db.create_task(&source, draft()).await.unwrap().task;
    let child = db.create_task(&source, draft()).await.unwrap().task;
    server.sync(&db).await.unwrap();
    server.sync(&peer).await.unwrap();
    db.move_tasks(
        &source,
        MoveTasksInput {
            task_ids: vec![parent.id.clone(), child.id.clone()],
            target_workspace: target.clone(),
            target_project: "Target".into(),
        },
    )
    .await
    .unwrap();
    db.add_task_to_epic(&target, &child.id, &parent.id)
        .await
        .unwrap();
    db.remove_task_from_epic(&target, &child.id, &parent.id)
        .await
        .unwrap();
    db.update_task(
        &target,
        &child.id,
        TaskUpdate {
            is_epic: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    db.mutate_tasks(
        &target,
        vec![(
            parent.id.clone(),
            TaskUpdate {
                title: Some("Undo me".into()),
                ..Default::default()
            },
        )],
        aven_core::undo::UndoContext::Tui,
    )
    .await
    .unwrap();
    server.sync(&db).await.unwrap();
    server.sync(&peer).await.unwrap();
    assert!(
        db.apply_latest_tui_undo(&target.id)
            .await
            .unwrap()
            .is_some()
    );
    server.sync(&db).await.unwrap();
    server.sync(&peer).await.unwrap();
    for db in [&db, &peer] {
        let snapshot = db.export_data("now".into()).await.unwrap();
        assert!(snapshot.tables.task_epic_links.is_empty());
        assert_eq!(
            snapshot
                .tables
                .tasks
                .iter()
                .find(|t| t.id == child.id)
                .unwrap()
                .is_epic,
            1
        );
        assert_eq!(
            snapshot
                .tables
                .tasks
                .iter()
                .find(|t| t.id == parent.id)
                .unwrap()
                .title,
            "Original title"
        );
        assert_eq!(db.sync_persistence_status().await.unwrap().conflicts, 0);
    }
}

#[tokio::test]
async fn pending_related_commands_survive_single_move_acknowledgement() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("first.sqlite"))
        .await
        .unwrap();
    let peer = Database::open(&dir.path().join("peer.sqlite"))
        .await
        .unwrap();
    let server = EncryptedSyncServer::new().await;
    let source = db.list_workspaces().await.unwrap().remove(0);
    db.create_project(&source, "Source", None).await.unwrap();
    let target = db.create_workspace("Target").await.unwrap();
    db.create_project(&target, "Target", None).await.unwrap();
    let a = db.create_task(&source, draft()).await.unwrap().task;
    let b = db.create_task(&source, draft()).await.unwrap().task;
    server.sync(&db).await.unwrap();
    server.sync(&peer).await.unwrap();
    db.move_tasks(
        &source,
        MoveTasksInput {
            task_ids: vec![a.id.clone(), b.id.clone()],
            target_workspace: target.clone(),
            target_project: "Target".into(),
        },
    )
    .await
    .unwrap();
    db.add_task_related_link(&target, &a.id, &b.id)
        .await
        .unwrap();
    db.remove_task_related_link(&target, &a.id, &b.id)
        .await
        .unwrap();
    assert!(server.push_one(&db).await.unwrap());
    server.sync(&db).await.unwrap();
    server.sync(&peer).await.unwrap();
    for db in [&db, &peer] {
        let snapshot = db.export_data("now".into()).await.unwrap();
        assert_eq!(snapshot.tables.task_related_links.len(), 1);
        assert_eq!(snapshot.tables.task_related_links[0].linked, 0);
    }
}

#[tokio::test]
async fn epic_removal_then_demotion_matches_combined_and_split_pages() {
    for page_size in [1, 256] {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("first.sqlite"))
            .await
            .unwrap();
        let peer = Database::open(&dir.path().join("peer.sqlite"))
            .await
            .unwrap();
        let server = EncryptedSyncServer::new().await;
        let source = db.list_workspaces().await.unwrap().remove(0);
        db.create_project(&source, "Source", None).await.unwrap();
        let target = db.create_workspace("Target").await.unwrap();
        db.create_project(&target, "Target", None).await.unwrap();
        let a = db.create_task(&source, draft()).await.unwrap().task;
        let b = db.create_task(&source, draft()).await.unwrap().task;
        db.add_task_to_epic(&source, &a.id, &b.id).await.unwrap();
        server.sync(&db).await.unwrap();
        server.sync(&peer).await.unwrap();
        db.move_tasks(
            &source,
            MoveTasksInput {
                task_ids: vec![b.id.clone()],
                target_workspace: target.clone(),
                target_project: "Target".into(),
            },
        )
        .await
        .unwrap();
        server.sync(&db).await.unwrap();
        server.sync(&peer).await.unwrap();
        db.remove_task_from_epic(&target, &a.id, &b.id)
            .await
            .unwrap();
        db.update_task(
            &target,
            &b.id,
            TaskUpdate {
                is_epic: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        while server.push_one(&db).await.unwrap() {}
        while server.pull_one(&peer, page_size).await.unwrap() {}
        for db in [&db, &peer] {
            let snapshot = db.export_data("now".into()).await.unwrap();
            assert!(snapshot.tables.task_epic_links.is_empty());
            assert_eq!(
                snapshot
                    .tables
                    .tasks
                    .iter()
                    .find(|t| t.id == b.id)
                    .unwrap()
                    .is_epic,
                0
            );
            assert_eq!(db.sync_persistence_status().await.unwrap().conflicts, 0);
        }
    }
}

#[tokio::test]
async fn earlier_offline_metadata_edit_does_not_leave_a_provisional_move_conflict() {
    use aven_core::metadata::TaskMetadataInput;
    for page_size in [1, 256] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("first.sqlite");
        let peer_path = dir.path().join("peer.sqlite");
        let db = Database::open(&path).await.unwrap();
        let peer = Database::open(&peer_path).await.unwrap();
        let server = EncryptedSyncServer::new().await;
        let source = db.list_workspaces().await.unwrap().remove(0);
        db.create_project(&source, "Source", None).await.unwrap();
        let target = db.create_workspace("Target").await.unwrap();
        db.create_project(&target, "Target", None).await.unwrap();
        let input = |value: &str| TaskMetadataInput {
            expected_field_id: None,
            key: "owner".into(),
            value: value.into(),
        };
        let mut d = draft();
        d.metadata = vec![input("A")];
        let task = db.create_task(&source, d).await.unwrap().task;
        server.sync(&db).await.unwrap();
        server.sync(&peer).await.unwrap();
        db.move_tasks(
            &source,
            MoveTasksInput {
                task_ids: vec![task.id.clone()],
                target_workspace: target.clone(),
                target_project: "Target".into(),
            },
        )
        .await
        .unwrap();
        db.update_task(
            &target,
            &task.id,
            TaskUpdate {
                set_metadata: vec![input("B")],
                ..Default::default()
            },
        )
        .await
        .unwrap();
        peer.update_task(
            &source,
            &task.id,
            TaskUpdate {
                set_metadata: vec![input("C")],
                ..Default::default()
            },
        )
        .await
        .unwrap();
        while server.push_one(&peer).await.unwrap() {}
        while server.push_one(&db).await.unwrap() {}
        while server.pull_one(&db, page_size).await.unwrap() {}
        server.sync(&peer).await.unwrap();
        drop(db);
        drop(peer);
        for path in [&path, &peer_path] {
            let db = Database::open(path).await.unwrap();
            assert_eq!(
                db.task_metadata(&target.id, &task.id).await.unwrap()[0].value,
                "B"
            );
            assert_eq!(db.sync_persistence_status().await.unwrap().conflicts, 0);
        }
    }
}

#[tokio::test]
async fn replay_keeps_deleted_catalogs_and_later_task_timestamp() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("first.sqlite"))
        .await
        .unwrap();
    let peer = Database::open(&dir.path().join("peer.sqlite"))
        .await
        .unwrap();
    let server = EncryptedSyncServer::new().await;
    let source = db.list_workspaces().await.unwrap().remove(0);
    db.create_project(&source, "Source", None).await.unwrap();
    let target = db.create_workspace("Target").await.unwrap();
    db.create_project(&target, "Target", None).await.unwrap();
    db.create_project(&target, "Other", None).await.unwrap();
    db.create_label(&source, "tag").await.unwrap();
    let mut d = draft();
    d.labels = vec!["tag".into()];
    let task = db.create_task(&source, d).await.unwrap().task;
    server.sync(&db).await.unwrap();
    server.sync(&peer).await.unwrap();
    db.move_tasks(
        &source,
        MoveTasksInput {
            task_ids: vec![task.id.clone()],
            target_workspace: target.clone(),
            target_project: "Target".into(),
        },
    )
    .await
    .unwrap();
    server.sync(&db).await.unwrap();
    server.sync(&peer).await.unwrap();
    db.delete_label(&target, "tag").await.unwrap();
    db.update_task(
        &target,
        &task.id,
        TaskUpdate {
            project: Some("Other".into()),
            title: Some("Later title".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let updated = db
        .export_data("now".into())
        .await
        .unwrap()
        .tables
        .tasks
        .into_iter()
        .find(|t| t.id == task.id)
        .unwrap()
        .updated_at;
    db.delete_project(&target, "Target").await.unwrap();
    server.sync(&db).await.unwrap();
    server.sync(&peer).await.unwrap();
    for db in [&db, &peer] {
        assert!(db.list_labels(&target.id, None).await.unwrap().is_empty());
        let snapshot = db.export_data("now".into()).await.unwrap();
        assert_eq!(
            snapshot
                .tables
                .projects
                .iter()
                .find(|p| p.workspace_id == target.id && p.key == "target")
                .unwrap()
                .deleted,
            1
        );
        assert!(
            snapshot
                .tables
                .tasks
                .iter()
                .find(|t| t.id == task.id)
                .unwrap()
                .updated_at
                >= updated
        );
    }
}

#[tokio::test]
async fn concurrent_moves_and_a_return_move_converge() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("first.sqlite"))
        .await
        .unwrap();
    let peer = Database::open(&dir.path().join("peer.sqlite"))
        .await
        .unwrap();
    let server = EncryptedSyncServer::new().await;
    let source = db.list_workspaces().await.unwrap().remove(0);
    db.create_project(&source, "Source", None).await.unwrap();
    let target = db.create_workspace("Target").await.unwrap();
    db.create_project(&target, "Target", None).await.unwrap();
    let other = db.create_workspace("Other").await.unwrap();
    db.create_project(&other, "Other", None).await.unwrap();
    let task = db.create_task(&source, draft()).await.unwrap().task;
    server.sync(&db).await.unwrap();
    server.sync(&peer).await.unwrap();
    db.move_tasks(
        &source,
        MoveTasksInput {
            task_ids: vec![task.id.clone()],
            target_workspace: target,
            target_project: "Target".into(),
        },
    )
    .await
    .unwrap();
    peer.move_tasks(
        &source,
        MoveTasksInput {
            task_ids: vec![task.id.clone()],
            target_workspace: other.clone(),
            target_project: "Other".into(),
        },
    )
    .await
    .unwrap();
    server.sync(&db).await.unwrap();
    server.sync(&peer).await.unwrap();
    server.sync(&db).await.unwrap();
    for db in [&db, &peer] {
        assert_eq!(
            db.export_data("now".into()).await.unwrap().tables.tasks[0].workspace_id,
            other.id
        );
    }
    db.move_tasks(
        &other,
        MoveTasksInput {
            task_ids: vec![task.id.clone()],
            target_workspace: source.clone(),
            target_project: "Source".into(),
        },
    )
    .await
    .unwrap();
    server.sync(&db).await.unwrap();
    server.sync(&peer).await.unwrap();
    for db in [&db, &peer] {
        assert_eq!(
            db.export_data("now".into()).await.unwrap().tables.tasks[0].workspace_id,
            source.id
        );
    }
}

#[tokio::test]
async fn metadata_replay_retains_a_genuine_post_move_conflict() {
    use aven_core::metadata::TaskMetadataInput;
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("first.sqlite"))
        .await
        .unwrap();
    let peer = Database::open(&dir.path().join("peer.sqlite"))
        .await
        .unwrap();
    let server = EncryptedSyncServer::new().await;
    let source = db.list_workspaces().await.unwrap().remove(0);
    db.create_project(&source, "Source", None).await.unwrap();
    let target = db.create_workspace("Target").await.unwrap();
    db.create_project(&target, "Target", None).await.unwrap();
    let input = |value: &str| TaskMetadataInput {
        expected_field_id: None,
        key: "owner".into(),
        value: value.into(),
    };
    let mut d = draft();
    d.metadata = vec![input("A")];
    let task = db.create_task(&source, d).await.unwrap().task;
    server.sync(&db).await.unwrap();
    server.sync(&peer).await.unwrap();
    db.move_tasks(
        &source,
        MoveTasksInput {
            task_ids: vec![task.id.clone()],
            target_workspace: target.clone(),
            target_project: "Target".into(),
        },
    )
    .await
    .unwrap();
    server.sync(&db).await.unwrap();
    server.sync(&peer).await.unwrap();
    for (db, value) in [(&db, "B"), (&peer, "C")] {
        db.update_task(
            &target,
            &task.id,
            TaskUpdate {
                set_metadata: vec![input(value)],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    server.sync(&db).await.unwrap();
    server.sync(&peer).await.unwrap();
    server.sync(&db).await.unwrap();
    for db in [&db, &peer] {
        assert_eq!(
            db.task_metadata(&target.id, &task.id).await.unwrap()[0].value,
            "B"
        );
        assert_eq!(db.sync_persistence_status().await.unwrap().conflicts, 1);
    }
}
