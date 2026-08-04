use super::*;

#[tokio::test]
async fn detail_back_and_forward_round_trip_linked_task_state() {
    let mut app = test_app().await;
    let parent = create_and_select_task(&mut app, test_task_draft("Parent")).await;
    let parent_id = app.store.tasks[parent].task.id.clone();
    let child = create_and_select_task(&mut app, test_task_draft("Child")).await;
    let child_id = app.store.tasks[child].task.id.clone();
    let parent = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == parent_id)
        .unwrap();
    app.list.select_task(Some(parent));
    app.show_detail(3);

    app.open_detail_task(&child_id, 3).await;
    app.detail.state_mut().unwrap().set_scroll(7);
    app.dispatch_key(key(KeyCode::Char('g')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('[')), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(
        app.store.tasks[app.list.selected_task().unwrap()].task.id,
        parent_id
    );
    assert_eq!(app.detail.state().unwrap().scroll(), 3);

    app.dispatch_key(key(KeyCode::Char('g')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char(']')), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(
        app.store.tasks[app.list.selected_task().unwrap()].task.id,
        child_id
    );
    assert_eq!(app.detail.state().unwrap().scroll(), 7);
}

#[tokio::test]
async fn empty_detail_forward_keeps_detail_open() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Task")).await;
    app.show_detail(4);

    app.dispatch_key(key(KeyCode::Char('g')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char(']')), (80, 24).into())
        .await
        .unwrap();

    assert!(app.detail.is_active());
    assert_eq!(app.detail.state().unwrap().scroll(), 4);
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("no next detail navigation state")
    );
}

#[tokio::test]
async fn fresh_linked_navigation_after_back_clears_detail_forward_history() {
    let mut app = test_app().await;
    let parent = create_and_select_task(&mut app, test_task_draft("Parent")).await;
    let child = create_and_select_task(&mut app, test_task_draft("Child")).await;
    let child_id = app.store.tasks[child].task.id.clone();
    let sibling = create_and_select_task(&mut app, test_task_draft("Sibling")).await;
    let sibling_id = app.store.tasks[sibling].task.id.clone();
    app.list.select_task(Some(parent));
    app.show_detail(0);
    app.open_detail_task(&child_id, 0).await;
    app.navigate_back_from_detail().await.unwrap();

    app.open_detail_task(&sibling_id, 0).await;
    app.navigate_forward_from_detail().await.unwrap();

    assert_eq!(
        app.store.tasks[app.list.selected_task().unwrap()].task.id,
        sibling_id
    );
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("no next detail navigation state")
    );
}

#[tokio::test]
async fn detail_back_returns_from_epic_child_to_parent_detail() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let (parent_id, child_ids) =
        create_epic_with_children(&mut app, &pool, "Parent epic", &["Child task"]).await;
    let child_id = child_ids[0].clone();
    app.store.refresh(Some(&parent_id)).await.unwrap();
    let parent_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == parent_id)
        .unwrap();
    app.list.select_task(Some(parent_index));
    app.show_detail(3);

    let parent_item = app.store.selected_task(app.list.selected_task()).unwrap();
    let click = (0..24)
        .flat_map(|row| (0..80).map(move |column| (column, row)))
        .find(|(column, row)| {
            crate::tui::ui::detail_child_task_at_position(parent_item, 80, 24, *column, *row, 3)
                .is_some_and(|hit| hit.task_id == child_id)
        })
        .map(|(column, row)| left_click(column, row))
        .expect("expected child task hit target");

    app.dispatch_mouse(click, (80, 24).into()).await.unwrap();
    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.id, child_id);
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());

    app.dispatch_key(key(KeyCode::Char('g')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('[')), (80, 24).into())
        .await
        .unwrap();

    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.id, parent_id);
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn detail_back_escape_restores_parent_scroll_and_child_focus() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let (parent_id, child_ids) =
        create_epic_with_children(&mut app, &pool, "Parent epic", &["Child task"]).await;
    let child_id = child_ids[0].clone();
    app.store.refresh(Some(&parent_id)).await.unwrap();
    app.list.select_task(Some(
        app.store
            .tasks
            .iter()
            .position(|item| item.task.id == parent_id)
            .unwrap(),
    ));
    app.show_detail(7);
    if app.detail.is_inactive() {
        app.show_detail(0);
    }
    app.detail.state_mut().unwrap().focused_target = Some(DetailTargetId::Task {
        section: DetailSection::EpicChildren,
        task_id: child_id.clone(),
    });

    app.open_detail_task(&child_id, 7).await;
    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();

    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.id, parent_id);
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(
        app.detail
            .state_mut()
            .unwrap()
            .focused_target
            .as_ref()
            .and_then(DetailTargetId::task_id),
        Some(&child_id)
    );
}

#[tokio::test]
async fn detail_back_cycle_unwinds_one_frame_per_escape() {
    let mut app = test_app().await;
    let first = create_and_select_task(&mut app, test_task_draft("First")).await;
    let first_id = app.store.tasks[first].task.id.clone();
    let second = create_and_select_task(&mut app, test_task_draft("Second")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    app.push_detail_navigation_state(detail_navigation_state(&app, first_id.clone(), 2));
    app.push_detail_navigation_state(detail_navigation_state(&app, second_id.clone(), 4));
    app.list.select_task(Some(first));
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(
        app.store.tasks[app.list.selected_task().unwrap()].task.id,
        second_id
    );
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());

    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(
        app.store.tasks[app.list.selected_task().unwrap()].task.id,
        first_id
    );
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());

    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_inactive());
}

#[tokio::test]
async fn detail_back_escape_leaves_focus_before_popping_history() {
    let mut app = test_app().await;
    let parent = create_and_select_task(&mut app, test_task_draft("Parent")).await;
    let parent_id = app.store.tasks[parent].task.id.clone();
    let child = create_and_select_task(&mut app, test_task_draft("Child")).await;
    app.store.tasks[child].attachments = vec![test_attachment(
        "FOCUSEDIMAGE",
        "image/png",
        true,
        Some((640, 480)),
    )];
    app.list.select_task(Some(child));
    app.show_detail(5);
    if app.detail.is_inactive() {
        app.show_detail(0);
    }
    app.detail.state_mut().unwrap().focused_target = Some(DetailTargetId::Attachment {
        attachment_id: "FOCUSEDIMAGE".to_string(),
    });
    app.push_detail_navigation_state(detail_navigation_state(&app, parent_id.clone(), 3));

    app.dispatch_key(key(KeyCode::Esc), (100, 30).into())
        .await
        .unwrap();
    assert!(app.detail.state_mut().unwrap().focused_target.is_none());
    assert_eq!(
        app.store.tasks[app.list.selected_task().unwrap()]
            .task
            .title,
        "Child"
    );
    assert!(app.detail_has_parent());

    app.dispatch_key(key(KeyCode::Esc), (100, 30).into())
        .await
        .unwrap();
    assert_eq!(
        app.store.tasks[app.list.selected_task().unwrap()].task.id,
        parent_id
    );
}

#[tokio::test]
async fn detail_back_shortcut_works_while_attachment_is_focused() {
    let mut app = test_app().await;
    let parent = create_and_select_task(&mut app, test_task_draft("Parent")).await;
    let parent_id = app.store.tasks[parent].task.id.clone();
    let child = create_and_select_task(&mut app, test_task_draft("Child")).await;
    app.store.tasks[child].attachments = vec![test_attachment(
        "FOCUSEDIMAGE",
        "image/png",
        true,
        Some((640, 480)),
    )];
    app.list.select_task(Some(child));
    app.show_detail(0);
    if app.detail.is_inactive() {
        app.show_detail(0);
    }
    app.detail.state_mut().unwrap().focused_target = Some(DetailTargetId::Attachment {
        attachment_id: "FOCUSEDIMAGE".to_string(),
    });
    app.push_detail_navigation_state(detail_navigation_state(&app, parent_id.clone(), 6));

    app.dispatch_key(key(KeyCode::Char('g')), (100, 30).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('[')), (100, 30).into())
        .await
        .unwrap();

    assert_eq!(
        app.store.tasks[app.list.selected_task().unwrap()].task.id,
        parent_id
    );
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn detail_back_skips_unavailable_history_entries() {
    let mut app = test_app().await;
    let parent = create_and_select_task(&mut app, test_task_draft("Parent")).await;
    let parent_id = app.store.tasks[parent].task.id.clone();
    create_and_select_task(&mut app, test_task_draft("Child")).await;
    app.push_detail_navigation_state(detail_navigation_state(&app, parent_id.clone(), 8));
    app.push_detail_navigation_state(detail_navigation_state(&app, crate::ids::TaskId::new(), 4));
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(
        app.store.tasks[app.list.selected_task().unwrap()].task.id,
        parent_id
    );
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Warning));
}

#[tokio::test]
async fn detail_back_exhausted_unavailable_history_closes_detail() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Child")).await;
    app.push_detail_navigation_state(detail_navigation_state(&app, crate::ids::TaskId::new(), 4));
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_inactive());
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Warning));
}

#[tokio::test]
async fn detail_back_q_closes_session_and_enter_keeps_root_open() {
    let mut app = test_app().await;
    let root = create_and_select_task(&mut app, test_task_draft("Root")).await;
    let root_id = app.store.tasks[root].task.id.clone();
    create_and_select_task(&mut app, test_task_draft("Child")).await;
    app.push_detail_navigation_state(detail_navigation_state(&app, root_id, 3));
    app.show_detail(5);

    app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());

    app.dispatch_key(key(KeyCode::Char('q')), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_inactive());
}

#[tokio::test]
async fn detail_back_attachment_preview_escape_precedes_history() {
    let mut app = test_app().await;
    let parent = create_and_select_task(&mut app, test_task_draft("Parent")).await;
    let parent_id = app.store.tasks[parent].task.id.clone();
    create_and_select_task(&mut app, test_task_draft("Child")).await;
    app.push_detail_navigation_state(detail_navigation_state(&app, parent_id.clone(), 9));
    if app.detail.is_inactive() {
        app.show_detail(0);
    }
    app.detail.state_mut().unwrap().focused_target = Some(DetailTargetId::Attachment {
        attachment_id: "PREVIEWIMAGE".to_string(),
    });
    app.overlay = Some(OverlayState::AttachmentPreview {
        attachment_id: "PREVIEWIMAGE".to_string(),
        scroll: 4,
    });

    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert!(app.detail_has_parent());

    if app.detail.is_inactive() {
        app.show_detail(0);
    }
    app.detail.state_mut().unwrap().focused_target = None;
    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(
        app.store.tasks[app.list.selected_task().unwrap()].task.id,
        parent_id
    );
}

#[tokio::test]
async fn detail_back_refresh_preserves_usable_parent_history() {
    let mut app = test_app().await;
    let parent = create_and_select_task(&mut app, test_task_draft("Parent")).await;
    let parent_id = app.store.tasks[parent].task.id.clone();
    let child = create_and_select_task(&mut app, test_task_draft("Child")).await;
    let child_id = app.store.tasks[child].task.id.clone();
    app.push_detail_navigation_state(detail_navigation_state(&app, parent_id.clone(), 5));
    app.show_detail(0);

    app.refresh().await.unwrap();
    assert_eq!(
        app.store.tasks[app.list.selected_task().unwrap()].task.id,
        child_id
    );
    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(
        app.store.tasks[app.list.selected_task().unwrap()].task.id,
        parent_id
    );
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn detail_back_root_escape_closes_direct_detail() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Root")).await;
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_inactive());
}

#[tokio::test]
async fn detail_back_q_prevents_history_leaking_into_later_session() {
    let mut app = test_app().await;
    let parent = create_and_select_task(&mut app, test_task_draft("Parent")).await;
    let parent_id = app.store.tasks[parent].task.id.clone();
    create_and_select_task(&mut app, test_task_draft("Child")).await;
    app.push_detail_navigation_state(detail_navigation_state(&app, parent_id, 3));
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Char('q')), (80, 24).into())
        .await
        .unwrap();
    app.activate_or_toggle_detail().await.unwrap();
    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_inactive());
}
