use super::*;

#[tokio::test]
async fn explicit_workspace_payloads_pair_id_and_key() {
    let (_dir, pool, _store) = test_store_with_pool().await;
    let mut conn = pool.acquire().await.unwrap();
    let other = crate::workspaces::create_workspace(&mut conn, "Client Work")
        .await
        .unwrap();

    crate::operations::create_label_operation(&mut conn, &other, "Needs Review")
        .await
        .unwrap();
    assert_workspace_payload(
        &latest_payload(&mut conn, "label", "create_label").await,
        &other,
    );

    let task = crate::operations::create_task(
        &mut conn,
        &other,
        TaskDraft {
            title: "Scoped task".to_string(),
            project: Some("Mobile App".to_string()),
            labels: vec!["Needs Review".to_string()],
            ..task_draft("")
        },
    )
    .await
    .unwrap()
    .task;
    assert_workspace_payload(
        &latest_payload(&mut conn, "project", "create_project").await,
        &other,
    );
    assert_workspace_payload(
        &latest_payload(&mut conn, "task", "create_task").await,
        &other,
    );

    crate::operations::create_label_operation(&mut conn, &other, "Docs")
        .await
        .unwrap();
    crate::operations::update_task_labels_in_workspace(
        &mut conn,
        &other.id,
        &task.id,
        &[String::from("Docs")],
        &[String::from("Needs Review")],
    )
    .await
    .unwrap();
    assert_workspace_payload(
        &latest_payload(&mut conn, "task", "label_add").await,
        &other,
    );
    assert_workspace_payload(
        &latest_payload(&mut conn, "task", "label_remove").await,
        &other,
    );
}
