use std::path::{Path, PathBuf};

use aven_core::api::{CreateTask, Store};
use aven_core::choices::{TaskPriority, TaskStatus};
use aven_core::db::Database;
use aven_core::ids::{TaskId, WorkspaceId};
use aven_core::sync::wire::{
    MAX_PULL_BATCH, MAX_PUSH_BATCH, SYNC_PROTOCOL_VERSION, SyncRequest, SyncResponse,
};
use aven_core::sync::{ApplySyncPage, ServerSyncPage};

struct Replicas {
    directory: tempfile::TempDir,
    first: PathBuf,
    second: PathBuf,
    server: Database,
    workspace: WorkspaceId,
    child: TaskId,
    parents: [TaskId; 2],
}

impl Replicas {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.sqlite");
        let second = directory.path().join("second.sqlite");
        let server = Database::open(&directory.path().join("server.sqlite"))
            .await
            .unwrap();
        let store = Store::open(&first).await.unwrap();
        let workspace = store.resolve_workspace("default").await.unwrap().id;
        let mut tasks = Vec::new();
        for title in ["child", "parent one", "parent two"] {
            tasks.push(
                store
                    .create_task(
                        &workspace,
                        CreateTask {
                            title: title.to_string(),
                            description: String::new(),
                            project: "Core".to_string(),
                            status: TaskStatus::Inbox,
                            priority: TaskPriority::None,
                            available_at: None,
                            due_on: None,
                            metadata: Vec::new(),
                        },
                    )
                    .await
                    .unwrap()
                    .id,
            );
        }
        let child = tasks.remove(0);
        tasks.sort();
        let replicas = Self {
            directory,
            first,
            second,
            server,
            workspace,
            child,
            parents: tasks.try_into().unwrap(),
        };
        // Both parents are promoted in the common history, independently of the race.
        for parent in &replicas.parents {
            replicas.add(&replicas.first, parent).await;
            replicas.remove(&replicas.first, parent).await;
        }
        replicas.settle(MAX_PUSH_BATCH, MAX_PULL_BATCH).await;
        replicas.assert_all(None).await;
        replicas
    }

    async fn add(&self, path: &Path, parent: &TaskId) {
        let db = Database::open(path).await.unwrap();
        let workspace = db.workspace_for_id(&self.workspace).await.unwrap();
        assert!(
            db.add_task_to_epic(&workspace, &self.child, parent)
                .await
                .unwrap()
                .changed
        );
    }

    async fn remove(&self, path: &Path, parent: &TaskId) {
        let db = Database::open(path).await.unwrap();
        let workspace = db.workspace_for_id(&self.workspace).await.unwrap();
        assert!(
            db.remove_task_from_epic(&workspace, &self.child, parent)
                .await
                .unwrap()
                .changed
        );
    }

    async fn linked() -> Self {
        let replicas = Self::new().await;
        replicas.add(&replicas.first, &replicas.parents[0]).await;
        replicas.settle(MAX_PUSH_BATCH, MAX_PULL_BATCH).await;
        replicas.assert_all(Some(&replicas.parents[0])).await;
        replicas
    }

    async fn settle(&self, push_limit: usize, pull_limit: u32) {
        drain(&self.first, &self.server, push_limit, pull_limit).await;
        drain(&self.second, &self.server, push_limit, pull_limit).await;
        drain(&self.first, &self.server, push_limit, pull_limit).await;
    }

    async fn assert_parent(&self, path: &Path, expected: Option<&TaskId>) {
        // Each read opens persisted state without retaining a replica handle.
        let store = Store::open(path).await.unwrap();
        let detail = store
            .ios_task_detail(&self.workspace, &self.child)
            .await
            .unwrap();
        assert_eq!(
            detail.epic_parent.as_ref().map(|parent| &parent.task_id),
            expected,
            "child parent at {}",
            path.display()
        );
        assert!(detail.blocked_by.is_empty());
        assert!(detail.blocks.is_empty());
        for parent in &self.parents {
            let detail = store
                .ios_task_detail(&self.workspace, parent)
                .await
                .unwrap();
            let children: Vec<_> = detail
                .epic_children
                .iter()
                .map(|child| &child.task_id)
                .collect();
            let expected_children = if expected == Some(parent) {
                vec![&self.child]
            } else {
                Vec::new()
            };
            assert_eq!(
                children,
                expected_children,
                "parent children at {}",
                path.display()
            );
            assert!(detail.is_epic, "membership replay preserves promotion");
        }
    }

    async fn assert_all(&self, expected: Option<&TaskId>) {
        for path in [&self.first, &self.second] {
            let db = Database::open(path).await.unwrap();
            let facts = db.ios_sync_facts().await.unwrap();
            assert_eq!(facts.pending_changes, 0, "{}: {facts:?}", path.display());
            assert!(facts.metadata_caught_up, "{}: {facts:?}", path.display());
            drop(db);
            self.assert_parent(path, expected).await;
        }
    }

    async fn assert_quiet(&self, expected: Option<&TaskId>) {
        self.assert_all(expected).await;
        for path in [&self.first, &self.second] {
            let response = exchange(path, &self.server, MAX_PUSH_BATCH, MAX_PULL_BATCH).await;
            assert!(response.push_acks.is_empty());
            assert!(response.changes.is_empty());
            assert!(!response.has_more);
        }
        self.assert_all(expected).await;
    }
}

async fn prepare(path: &Path, push_limit: usize, pull_limit: u32) -> SyncRequest {
    Database::open(path)
        .await
        .unwrap()
        .prepare_client_sync_page("https://sync.test".to_string(), push_limit, pull_limit)
        .await
        .unwrap()
        .request
}

async fn respond(server: &Database, request: &SyncRequest) -> SyncResponse {
    let result = server
        .persist_server_sync_page(ServerSyncPage {
            request: request.clone(),
        })
        .await
        .unwrap();
    SyncResponse {
        protocol_version: SYNC_PROTOCOL_VERSION,
        cursor: result
            .changes
            .last()
            .and_then(|change| change.server_seq)
            .unwrap_or(request.after),
        has_more: result.has_more,
        push_acks: result.push_acks,
        changes: result.changes,
    }
}

async fn apply(path: &Path, request: SyncRequest, response: SyncResponse) {
    Database::open(path)
        .await
        .unwrap()
        .apply_client_sync_page(ApplySyncPage {
            request,
            response,
            attempted_at: "2026-09-06T00:00:00Z".to_string(),
            previous_pushed: 0,
            previous_pulled: 0,
        })
        .await
        .unwrap();
}

async fn exchange(
    path: &Path,
    server: &Database,
    push_limit: usize,
    pull_limit: u32,
) -> SyncResponse {
    let request = prepare(path, push_limit, pull_limit).await;
    let response = respond(server, &request).await;
    let decoded = serde_json::from_slice(&serde_json::to_vec(&response).unwrap()).unwrap();
    apply(path, request, decoded).await;
    response
}

async fn drain(path: &Path, server: &Database, push_limit: usize, pull_limit: u32) {
    for _ in 0..64 {
        let response = exchange(path, server, push_limit, pull_limit).await;
        let facts = Database::open(path)
            .await
            .unwrap()
            .ios_sync_facts()
            .await
            .unwrap();
        if !response.has_more && facts.pending_changes == 0 {
            return;
        }
    }
    panic!("sync failed to drain at {}", path.display());
}

async fn remove_readd_race(reverse_upload: bool, push_limit: usize, pull_limit: u32) {
    let replicas = Replicas::linked().await;
    let parent = &replicas.parents[0];
    replicas.remove(&replicas.first, parent).await;
    replicas.remove(&replicas.second, parent).await;
    replicas.add(&replicas.second, parent).await;
    let order = if reverse_upload {
        [&replicas.second, &replicas.first]
    } else {
        [&replicas.first, &replicas.second]
    };
    for path in order {
        drain(path, &replicas.server, push_limit, pull_limit).await;
    }
    replicas.settle(push_limit, pull_limit).await;
    // Server order is remove/remove/add or remove/add/remove, respectively.
    let expected = if reverse_upload { None } else { Some(parent) };
    replicas.assert_quiet(expected).await;
}

#[tokio::test]
async fn remove_readd_converges_in_upload_order() {
    remove_readd_race(false, MAX_PUSH_BATCH, MAX_PULL_BATCH).await;
}

#[tokio::test]
async fn remove_readd_converges_in_reverse_upload_order() {
    remove_readd_race(true, MAX_PUSH_BATCH, MAX_PULL_BATCH).await;
}

#[tokio::test]
async fn remove_readd_converges_with_one_change_pull_pages() {
    remove_readd_race(false, MAX_PUSH_BATCH, 1).await;
}

#[tokio::test]
async fn remove_readd_converges_with_reverse_one_change_pull_pages() {
    remove_readd_race(true, MAX_PUSH_BATCH, 1).await;
}

#[tokio::test]
async fn remove_readd_converges_with_split_push_batches() {
    remove_readd_race(false, 1, 1).await;
    remove_readd_race(true, 1, 1).await;
}

#[tokio::test]
async fn competing_parents_choose_minimum_without_resurrecting_loser() {
    for reverse_upload in [false, true] {
        let replicas = Replicas::new().await;
        let [winner, loser] = &replicas.parents;
        replicas.add(&replicas.first, winner).await;
        replicas.add(&replicas.second, loser).await;
        let order = if reverse_upload {
            [&replicas.second, &replicas.first]
        } else {
            [&replicas.first, &replicas.second]
        };
        for path in order {
            drain(path, &replicas.server, 1, 1).await;
        }
        replicas.settle(1, 1).await;
        replicas.assert_all(Some(winner)).await;
        replicas.remove(&replicas.first, winner).await;
        replicas.settle(1, 1).await;
        replicas.assert_quiet(None).await;
    }
}

#[tokio::test]
async fn stale_removal_of_losing_parent_preserves_winner() {
    let replicas = Replicas::new().await;
    let [winner, loser] = &replicas.parents;
    replicas.add(&replicas.first, winner).await;
    replicas.add(&replicas.second, loser).await;
    replicas.remove(&replicas.second, loser).await;
    replicas.settle(1, 1).await;
    replicas.assert_quiet(Some(winner)).await;
}

#[tokio::test]
async fn acknowledgement_beyond_cursor_preserves_write_after_prepare_and_reopen() {
    let replicas = Replicas::linked().await;
    let parent = &replicas.parents[0];
    replicas.remove(&replicas.first, parent).await;
    replicas.remove(&replicas.second, parent).await;
    drain(
        &replicas.first,
        &replicas.server,
        MAX_PUSH_BATCH,
        MAX_PULL_BATCH,
    )
    .await;

    let request = prepare(&replicas.second, MAX_PUSH_BATCH, 1).await;
    assert_eq!(request.changes.len(), 1);
    let response = respond(&replicas.server, &request).await;
    assert_eq!(response.changes.len(), 1);
    assert_eq!(response.push_acks.len(), 1);
    assert!(response.has_more);
    assert!(response.push_acks[0].server_seq > response.cursor);

    // This write is absent from the outstanding request and must remain optimistic.
    replicas.add(&replicas.second, parent).await;
    apply(&replicas.second, request, response).await;
    let db = Database::open(&replicas.second).await.unwrap();
    let facts = db.ios_sync_facts().await.unwrap();
    assert_eq!(facts.pending_changes, 1);
    assert!(!facts.metadata_caught_up);
    drop(db);
    replicas.assert_parent(&replicas.second, Some(parent)).await;

    replicas.settle(1, 1).await;
    replicas.assert_quiet(Some(parent)).await;
}

#[tokio::test]
async fn acknowledged_readd_beyond_cursor_is_visible_without_pending_writes() {
    let replicas = Replicas::linked().await;
    let parent = &replicas.parents[0];
    replicas.remove(&replicas.first, parent).await;
    replicas.remove(&replicas.second, parent).await;
    replicas.add(&replicas.second, parent).await;
    drain(
        &replicas.first,
        &replicas.server,
        MAX_PUSH_BATCH,
        MAX_PULL_BATCH,
    )
    .await;

    let request = prepare(&replicas.second, MAX_PUSH_BATCH, 1).await;
    assert_eq!(request.changes.len(), 2);
    let response = respond(&replicas.server, &request).await;
    assert_eq!(response.changes.len(), 1);
    assert_eq!(response.push_acks.len(), 2);
    assert!(response.has_more);
    assert!(
        response
            .push_acks
            .iter()
            .all(|ack| ack.server_seq > response.cursor)
    );
    apply(&replicas.second, request, response).await;

    let facts = Database::open(&replicas.second)
        .await
        .unwrap()
        .ios_sync_facts()
        .await
        .unwrap();
    assert_eq!(facts.pending_changes, 0);
    assert!(!facts.metadata_caught_up);
    replicas.assert_parent(&replicas.second, Some(parent)).await;
    replicas.settle(1, 1).await;
    replicas.assert_quiet(Some(parent)).await;
}

#[tokio::test]
async fn failed_epic_page_rolls_back_membership_acknowledgements_and_cursor() {
    let replicas = Replicas::linked().await;
    let parent = &replicas.parents[0];
    for path in [&replicas.first, &replicas.second] {
        replicas.remove(path, parent).await;
        replicas.add(path, parent).await;
    }
    drain(
        &replicas.first,
        &replicas.server,
        MAX_PUSH_BATCH,
        MAX_PULL_BATCH,
    )
    .await;
    let request = prepare(&replicas.second, MAX_PUSH_BATCH, MAX_PULL_BATCH).await;
    let response = respond(&replicas.server, &request).await;
    assert_eq!(request.changes.len(), 2);
    assert_eq!(response.changes.len(), 4);
    assert_eq!(response.changes[0].op_type, "epic_link_remove");
    assert_eq!(response.changes[1].op_type, "epic_link_add");
    let mut invalid: SyncResponse =
        serde_json::from_slice(&serde_json::to_vec(&response).unwrap()).unwrap();
    // A well-shaped but absent parent fails domain apply after the first removal.
    invalid.changes[1].payload["epic_task_id"] = serde_json::json!(TaskId::new());
    let db = Database::open(&replicas.second).await.unwrap();
    let before = db.sync_persistence_status().await.unwrap();
    let error = db
        .apply_client_sync_page(ApplySyncPage {
            request: request.clone(),
            response: invalid,
            attempted_at: "2026-09-06T00:00:00Z".to_string(),
            previous_pushed: 0,
            previous_pulled: 0,
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("epic-missing-task"), "{error:#}");
    assert_eq!(db.sync_persistence_status().await.unwrap(), before);
    drop(db);
    let retried = prepare(&replicas.second, MAX_PUSH_BATCH, MAX_PULL_BATCH).await;
    assert_eq!(
        serde_json::to_value(&retried).unwrap(),
        serde_json::to_value(&request).unwrap()
    );
    replicas.assert_parent(&replicas.second, Some(parent)).await;

    apply(&replicas.second, request, response).await;
    replicas.settle(MAX_PUSH_BATCH, MAX_PULL_BATCH).await;
    replicas.assert_quiet(Some(parent)).await;
}

#[tokio::test]
async fn full_history_import_recovers_missing_membership_without_new_edits() {
    let replicas = Replicas::linked().await;
    let source = Database::open(&replicas.first).await.unwrap();
    let mut export = source
        .export_data("2026-09-06T00:00:00Z".to_string())
        .await
        .unwrap();
    assert_eq!(export.tables.task_epic_links.len(), 1);
    // A complete operation history determines membership even if its snapshot diverges.
    export.tables.task_epic_links.clear();
    let imported_path = replicas.directory.path().join("full-history-import.sqlite");
    let imported = Database::open(&imported_path).await.unwrap();
    imported.validate_import_data(&export).await.unwrap();
    imported.import_data(&export).await.unwrap();
    drop(imported);
    replicas
        .assert_parent(&imported_path, Some(&replicas.parents[0]))
        .await;
    drain(&imported_path, &replicas.server, 1, 1).await;
    replicas
        .assert_parent(&imported_path, Some(&replicas.parents[0]))
        .await;
}

#[tokio::test]
async fn snapshot_only_import_retains_membership_across_writes_and_reopen() {
    let replicas = Replicas::linked().await;
    let source = Database::open(&replicas.first).await.unwrap();
    let mut export = source
        .export_data("2026-09-06T00:00:00Z".to_string())
        .await
        .unwrap();
    export.tables.changes.clear();
    export.tables.field_versions.clear();
    let imported_path = replicas.directory.path().join("snapshot-import.sqlite");
    let imported = Database::open(&imported_path).await.unwrap();
    imported.validate_import_data(&export).await.unwrap();
    imported.import_data(&export).await.unwrap();
    drop(imported);
    let parent = &replicas.parents[0];
    replicas.assert_parent(&imported_path, Some(parent)).await;
    replicas.remove(&imported_path, parent).await;
    replicas.assert_parent(&imported_path, None).await;

    let exported = Database::open(&imported_path)
        .await
        .unwrap()
        .export_data("2026-09-06T00:00:00Z".to_string())
        .await
        .unwrap();
    let restored_path = replicas.directory.path().join("snapshot-roundtrip.sqlite");
    let restored = Database::open(&restored_path).await.unwrap();
    restored.import_data(&exported).await.unwrap();
    drop(restored);
    replicas.assert_parent(&restored_path, None).await;
    replicas.add(&restored_path, parent).await;
    replicas.assert_parent(&restored_path, Some(parent)).await;
    replicas.remove(&restored_path, parent).await;
    replicas.assert_parent(&restored_path, None).await;
}
