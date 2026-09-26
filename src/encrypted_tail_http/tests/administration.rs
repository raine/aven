use super::membership::join;
use super::*;

/// Semantic rows that project, label and workspace administration can change.
async fn administered_state(db: &Database) -> Vec<String> {
    let mut c = aven_core::test_support::acquire(db).await.unwrap();
    let mut rows: Vec<String> = Vec::new();
    for sql in [
        "SELECT 'workspace ' || id || ' ' || key || ' ' || name FROM workspaces",
        "SELECT 'project ' || workspace_id || ' ' || id || ' ' || key || ' ' || name
                || ' ' || prefix || ' deleted=' || deleted FROM projects",
        "SELECT 'label ' || workspace_id || ' ' || name FROM labels",
        "SELECT 'task ' || t.workspace_id || ' ' || t.title || ' project=' || p.key
                || ' labels=' || COALESCE((SELECT group_concat(label, ',') FROM
                   (SELECT label FROM task_labels l WHERE l.workspace_id = t.workspace_id
                    AND l.task_id = t.id ORDER BY label)), '')
         FROM tasks t JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id",
    ] {
        rows.extend(
            sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(sql))
                .fetch_all(&mut *c)
                .await
                .unwrap(),
        );
    }
    rows.sort();
    rows
}

async fn task_labels_by_title(db: &Database, title: &str) -> Vec<String> {
    let mut c = aven_core::test_support::acquire(db).await.unwrap();
    sqlx::query_scalar(
        "SELECT l.label FROM task_labels l JOIN tasks t
           ON t.workspace_id = l.workspace_id AND t.id = l.task_id
         WHERE t.title = ? ORDER BY l.label",
    )
    .bind(title)
    .fetch_all(&mut *c)
    .await
    .unwrap()
}

#[tokio::test]
async fn project_label_and_workspace_administration_crosses_http_and_converges() {
    let f = fixture_with(with_relations()).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    converge(&f).await;

    // Every administration command queues ahead of a later ordinary task edit.
    let side = f.seed.create_workspace("Side").await.unwrap();
    f.seed.create_task(&side, draft("side task")).await.unwrap();
    f.seed.rename_workspace("side", "Lab").await.unwrap();
    f.seed
        .rename_project(&w, "app", "Core", Some("CORE"))
        .await
        .unwrap();
    let mut scratch = draft("scratch task");
    scratch.project = Some("scratch".into());
    f.seed.create_task(&w, scratch).await.unwrap();
    f.seed.delete_project(&w, "scratch").await.unwrap();
    f.seed
        .rename_label_with_tui_undo(&w, "tag", "topic")
        .await
        .unwrap();
    f.seed
        .delete_label_with_tui_undo(&w, "untouched")
        .await
        .unwrap();
    f.seed.apply_latest_tui_undo(&w.id).await.unwrap().unwrap();
    f.seed.create_label(&w, "doomed").await.unwrap();
    let doomed = f
        .seed
        .create_task(&w, draft("doomed label task"))
        .await
        .unwrap()
        .task;
    f.seed
        .update_task(
            &w,
            &doomed.id,
            TaskUpdate {
                add_labels: vec!["doomed".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    f.seed.delete_label(&w, "doomed").await.unwrap();
    let owner: aven_core::ids::TaskId = {
        let mut c = aven_core::test_support::acquire(&f.seed).await.unwrap();
        sqlx::query_scalar("SELECT id FROM tasks WHERE title = 'snapshot relation owner'")
            .fetch_one(&mut *c)
            .await
            .unwrap()
    };
    f.seed
        .update_task(
            &w,
            &owner,
            TaskUpdate {
                title: Some("owner after administration".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut emitted: Vec<String> = {
        let mut c = aven_core::test_support::acquire(&f.seed).await.unwrap();
        sqlx::query_scalar("SELECT DISTINCT op_type FROM changes WHERE server_seq IS NULL")
            .fetch_all(&mut *c)
            .await
            .unwrap()
    };
    emitted.sort();
    for op in [
        "create_workspace",
        "label_delete",
        "label_restore",
        "project_delete",
        "set_label_name",
        "set_project_metadata",
        "set_workspace_field",
    ] {
        assert!(emitted.iter().any(|e| e == op), "{op} not emitted");
    }

    converge(&f).await;
    let expected = administered_state(&f.seed).await;
    assert_eq!(administered_state(&f.peer).await, expected);
    for row in [
        format!("workspace {} lab Lab", side.id),
        format!("label {} topic", w.id),
        format!("label {} untouched", w.id),
        "task 0000000000000000 owner after administration project=core labels=topic,untouched"
            .into(),
        "task 0000000000000000 doomed label task project=app labels=".into(),
        format!("task {} side task project=app labels=", side.id),
    ] {
        assert!(expected.contains(&row), "{row} missing from {expected:#?}");
    }
    assert!(
        expected
            .iter()
            .any(|r| r.starts_with("project 0000000000000000 ")
                && r.ends_with(" core Core CORE deleted=0"))
    );
    assert!(
        expected
            .iter()
            .any(|r| r.contains(" scratch scratch ") && r.ends_with(" deleted=1"))
    );
    assert!(
        expected
            .iter()
            .any(|r| r.starts_with("task 0000000000000000 scratch task project=scratch"))
    );
    assert!(
        !expected
            .iter()
            .any(|r| r.starts_with("label ") && (r.ends_with(" tag") || r.ends_with(" doomed")))
    );
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        0
    );

    // Peer work behind the replayed administration reaches the seed.
    f.peer
        .update_task(
            &w,
            &owner,
            TaskUpdate {
                remove_labels: vec!["topic".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    converge(&f).await;
    assert_eq!(
        task_labels_by_title(&f.seed, "owner after administration").await,
        ["untouched"]
    );

    // A fresh installation catches up from the published prefix and the tail.
    let third = join(&f, "third", &f.seed, &f.seed_store).await;
    drain(&Client::new(&f.origin).unwrap(), &third.store, &third.db).await;
    let expected = administered_state(&f.seed).await;
    assert_eq!(administered_state(&f.peer).await, expected);
    assert_eq!(administered_state(&third.db).await, expected);
}

#[tokio::test]
async fn remote_label_commands_ordered_before_pending_deletion_or_rename_follow_it() {
    let f = fixture_with(with_relations()).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    converge(&f).await;
    let mut ids = Vec::new();
    for title in ["snapshot relation owner", "snapshot relation target"] {
        let mut c = aven_core::test_support::acquire(&f.seed).await.unwrap();
        let id: aven_core::ids::TaskId = sqlx::query_scalar("SELECT id FROM tasks WHERE title = ?")
            .bind(title)
            .fetch_one(&mut *c)
            .await
            .unwrap();
        ids.push(id);
    }
    // Offline peer work that the server orders before the seed's administration.
    f.peer.create_label(&w, "late").await.unwrap();
    for (task, label) in [(&ids[0], "late"), (&ids[1], "untouched")] {
        f.peer
            .update_task(
                &w,
                task,
                TaskUpdate {
                    add_labels: vec![label.into()],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    drain(&Client::new(&f.origin).unwrap(), &f.peer_store, &f.peer).await;
    f.seed.create_label(&w, "late").await.unwrap();
    f.seed
        .update_task(
            &w,
            &ids[0],
            TaskUpdate {
                add_labels: vec!["late".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    f.seed.delete_label(&w, "late").await.unwrap();
    f.seed
        .rename_label_with_tui_undo(&w, "untouched", "kept")
        .await
        .unwrap();
    converge(&f).await;
    for db in [&f.seed, &f.peer] {
        assert_eq!(
            task_labels_by_title(db, "snapshot relation owner").await,
            ["kept", "tag"]
        );
        assert_eq!(
            task_labels_by_title(db, "snapshot relation target").await,
            ["kept"]
        );
        let mut c = aven_core::test_support::acquire(db).await.unwrap();
        let labels: Vec<String> = sqlx::query_scalar("SELECT name FROM labels ORDER BY name")
            .fetch_all(&mut *c)
            .await
            .unwrap();
        assert_eq!(labels, ["kept", "tag"]);
    }
    assert_eq!(
        administered_state(&f.peer).await,
        administered_state(&f.seed).await
    );
}
