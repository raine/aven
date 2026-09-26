use std::path::{Path, PathBuf};

use aven_core::api::{CreateTask, Store};
use aven_core::choices::{TaskPriority, TaskStatus};
use aven_core::db::Database;
use aven_core::ids::{TaskId, WorkspaceId};
use aven_core::sync::encrypted_tail::PAGE_COUNT;
use aven_core::test_support::encrypted_sync::EncryptedSyncServer;

const ALL: usize = usize::MAX;

struct Replicas {
    directory: tempfile::TempDir,
    first: PathBuf,
    second: PathBuf,
    server: EncryptedSyncServer,
    workspace: WorkspaceId,
    child: TaskId,
    parents: [TaskId; 2],
}

impl Replicas {
    async fn new() -> Self {
        let replicas = Self::unsynced().await;
        replicas.settle(ALL, PAGE_COUNT).await;
        replicas.assert_all(None).await;
        replicas
    }

    /// Common history on the first replica, not yet associated with the server.
    async fn unsynced() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.sqlite");
        let second = directory.path().join("second.sqlite");
        let server = EncryptedSyncServer::new().await;
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
        replicas.settle(ALL, PAGE_COUNT).await;
        replicas.assert_all(Some(&replicas.parents[0])).await;
        replicas
    }

    async fn settle(&self, push_limit: usize, pull_limit: usize) {
        drain(&self.first, &self.server, push_limit, pull_limit).await;
        drain(&self.second, &self.server, push_limit, pull_limit).await;
        drain(&self.first, &self.server, push_limit, pull_limit).await;
    }

    async fn assert_parent(&self, path: &Path, expected: Option<&TaskId>) {
        // Each read opens persisted state without retaining a replica handle.
        let store = Store::open(path).await.unwrap();
        let detail = store
            .task_detail(&self.workspace, &self.child)
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
            let detail = store.task_detail(&self.workspace, parent).await.unwrap();
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
            let status = db.sync_persistence_status().await.unwrap();
            assert_eq!(status.pending_changes, 0, "{}: {status:?}", path.display());
            drop(db);
            self.assert_parent(path, expected).await;
        }
    }

    async fn assert_quiet(&self, expected: Option<&TaskId>) {
        self.assert_all(expected).await;
        for path in [&self.first, &self.second] {
            let db = Database::open(path).await.unwrap();
            assert!(!self.server.push_one(&db).await.unwrap());
            let page = self.server.fetch(&db, PAGE_COUNT).await.unwrap();
            assert!(page.records.is_empty());
            assert!(!page.has_more);
        }
        self.assert_all(expected).await;
    }
}

async fn drain(path: &Path, server: &EncryptedSyncServer, push_limit: usize, pull_limit: usize) {
    let db = Database::open(path).await.unwrap();
    server.sync_with(&db, push_limit, pull_limit).await.unwrap();
}

async fn pending(path: &Path) -> i64 {
    Database::open(path)
        .await
        .unwrap()
        .sync_persistence_status()
        .await
        .unwrap()
        .pending_changes
}

async fn remove_readd_race(reverse_upload: bool, push_limit: usize, pull_limit: usize) {
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
    remove_readd_race(false, ALL, PAGE_COUNT).await;
}

#[tokio::test]
async fn remove_readd_converges_in_reverse_upload_order() {
    remove_readd_race(true, ALL, PAGE_COUNT).await;
}

#[tokio::test]
async fn remove_readd_converges_with_one_change_pull_pages() {
    remove_readd_race(false, ALL, 1).await;
}

#[tokio::test]
async fn remove_readd_converges_with_reverse_one_change_pull_pages() {
    remove_readd_race(true, ALL, 1).await;
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
async fn acknowledgement_beyond_cursor_preserves_write_after_push_and_reopen() {
    let replicas = Replicas::linked().await;
    let parent = &replicas.parents[0];
    replicas.remove(&replicas.first, parent).await;
    replicas.remove(&replicas.second, parent).await;
    drain(&replicas.first, &replicas.server, ALL, PAGE_COUNT).await;

    // The acknowledged removal is sequenced after the unpulled remote removal.
    let second = Database::open(&replicas.second).await.unwrap();
    assert!(replicas.server.push_one(&second).await.unwrap());
    assert!(!replicas.server.push_one(&second).await.unwrap());
    // This write follows the acknowledged push and must remain optimistic.
    drop(second);
    replicas.add(&replicas.second, parent).await;
    let second = Database::open(&replicas.second).await.unwrap();
    assert!(replicas.server.pull_one(&second, 1).await.unwrap());
    drop(second);
    assert_eq!(pending(&replicas.second).await, 1);
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
    drain(&replicas.first, &replicas.server, ALL, PAGE_COUNT).await;

    let second = Database::open(&replicas.second).await.unwrap();
    assert!(replicas.server.push_one(&second).await.unwrap());
    assert!(replicas.server.push_one(&second).await.unwrap());
    assert!(!replicas.server.push_one(&second).await.unwrap());
    assert!(replicas.server.pull_one(&second, 1).await.unwrap());
    drop(second);

    assert_eq!(pending(&replicas.second).await, 0);
    replicas.assert_parent(&replicas.second, Some(parent)).await;
    replicas.settle(1, 1).await;
    replicas.assert_quiet(Some(parent)).await;
}

#[tokio::test]
async fn failed_epic_page_rolls_back_membership_and_cursor() {
    let replicas = Replicas::linked().await;
    let parent = &replicas.parents[0];
    for path in [&replicas.first, &replicas.second] {
        replicas.remove(path, parent).await;
        replicas.add(path, parent).await;
    }
    drain(&replicas.first, &replicas.server, ALL, PAGE_COUNT).await;
    let db = Database::open(&replicas.second).await.unwrap();
    let page = replicas.server.fetch(&db, PAGE_COUNT).await.unwrap();
    let ops: Vec<_> = page
        .records
        .iter()
        .map(|record| replicas.server.open(record).op_type)
        .collect();
    assert_eq!(ops, ["epic_link_remove", "epic_link_add"]);
    let mut invalid = page.clone();
    // A well-shaped but absent parent fails domain apply after the first removal.
    invalid.records[1] = replicas.server.reseal(&page.records[1], |change| {
        change.payload["epic_task_id"] = serde_json::json!(TaskId::new());
    });
    let before = db.sync_persistence_status().await.unwrap();
    let error = replicas.server.apply(&db, &invalid).await.unwrap_err();
    assert!(
        error.to_string().contains("encrypted-tail-apply"),
        "{error:#}"
    );
    assert_eq!(db.sync_persistence_status().await.unwrap(), before);
    assert_eq!(before.pending_changes, 2);
    drop(db);
    replicas.assert_parent(&replicas.second, Some(parent)).await;

    let db = Database::open(&replicas.second).await.unwrap();
    replicas.server.apply(&db, &page).await.unwrap();
    drop(db);
    replicas.settle(ALL, PAGE_COUNT).await;
    replicas.assert_quiet(Some(parent)).await;
}

#[tokio::test]
async fn full_history_import_recovers_missing_membership_without_new_edits() {
    // Export needs a source that was never associated with a server.
    let replicas = Replicas::unsynced().await;
    replicas.add(&replicas.first, &replicas.parents[0]).await;
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
    // Export needs a source that was never associated with a server.
    let replicas = Replicas::unsynced().await;
    replicas.add(&replicas.first, &replicas.parents[0]).await;
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
