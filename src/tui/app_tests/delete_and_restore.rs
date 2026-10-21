use super::*;

#[tokio::test]
async fn cancel_delete_task_leaves_task_unchanged() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Keep target")).await;

    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('D')).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

    assert!(app.overlay.is_none());
    assert!(!app.store.tasks[selected].task.deleted);
    assert!(app.notification.is_none());
}

#[tokio::test]
async fn confirm_delete_task_soft_deletes_selected_task() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Delete target")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    let display_ref = app.store.tasks[selected].display_ref.clone();

    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('D')).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Confirm(state))
            if matches!(&state.intent, ConfirmIntent::DeleteTasks { selection }
                if selection.single_id() == Some(&task_id))
                && state.title == DELETE_TASK_TITLE
                && state.prompt.contains(&display_ref)
                && state.prompt.contains("Delete target")
    ));
    assert!(!app.store.tasks[selected].task.deleted);
    app.handle_overlay_key(key(KeyCode::Char('y')))
        .await
        .unwrap();

    assert_eq!(app.list.selected_task(), Some(selected));
    assert_eq!(app.store.tasks.len(), 1);
    assert_eq!(app.store.tasks[selected].task.id, task_id);
    assert!(app.store.tasks[selected].task.deleted);
    assert!(
        app.store
            .load_task_item(&task_id)
            .await
            .unwrap()
            .unwrap()
            .task
            .deleted
    );
    assert!(!app.store.view_state.filter_modifiers.include_deleted);
    assert_eq!(
        toast_message(&app).as_deref(),
        Some(format!("deleted {display_ref} · u undo").as_str())
    );
}

#[tokio::test]
async fn delete_task_from_detail_returns_to_detail() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Detail delete target")).await;
    app.show_detail(7);

    app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('D')), (80, 24).into())
        .await
        .unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Confirm(ConfirmState {
            intent: ConfirmIntent::DeleteTasks { .. },
            ..
        }))
    ));
    assert!(app.view().detail_underlay);

    app.handle_overlay_key(key(KeyCode::Char('y')))
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert!(app.store.tasks[selected].task.deleted);
}

#[tokio::test]
async fn delete_project_picker_preselects_sidebar_project() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();
    app.list.focus_sidebar();
    let project_index = app
        .store
        .sidebar_entries
        .iter()
        .position(|entry| {
            entry.target
                == Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(
                    "mobile-app".to_string(),
                )))
        })
        .unwrap();
    app.list.select_sidebar(Some(project_index));

    app.execute(Action::BeginDeleteProject).await.unwrap();

    let Some(OverlayState::Picker(state)) = &app.overlay else {
        panic!("expected project picker");
    };
    assert_eq!(state.items[state.selected].value, "mobile-app");
}

#[tokio::test]
async fn command_project_picker_uses_captured_sidebar_project_after_focus_change() {
    let mut app = test_app().await;
    app.store
        .create_project("Project A".to_string())
        .await
        .unwrap();
    app.store
        .create_project("Project B".to_string())
        .await
        .unwrap();
    app.list.focus_sidebar();
    let project_index = |app: &App, key: &str| {
        app.store
            .sidebar_entries
            .iter()
            .position(|entry| {
                entry.target
                    == Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(
                        key.to_string(),
                    )))
            })
            .unwrap()
    };
    app.list
        .select_sidebar(Some(project_index(&app, "project-a")));

    app.begin_command().await;
    let command_overlay = app.overlay.take().expect("command overlay");
    app.list
        .select_sidebar(Some(project_index(&app, "project-b")));
    app.overlay = Some(command_overlay);
    type_chars(&mut app, "delete-project").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    let Some(OverlayState::Picker(state)) = &app.overlay else {
        panic!("expected project picker");
    };
    assert_eq!(state.items[state.selected].value, "project-a");
}

#[tokio::test]
async fn project_path_picker_preselects_sidebar_project() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();
    app.list.focus_sidebar();
    let project_index = app
        .store
        .sidebar_entries
        .iter()
        .position(|entry| {
            entry.target
                == Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(
                    "mobile-app".to_string(),
                )))
        })
        .unwrap();
    app.list.select_sidebar(Some(project_index));
    let scope = app.store.view_state.scope.clone();

    app.execute(Action::BeginAddProjectPath).await.unwrap();

    let Some(OverlayState::Picker(state)) = &app.overlay else {
        panic!("expected project picker");
    };
    assert_eq!(state.intent, PickerIntent::AddProjectPath);
    assert_eq!(state.title, "Add project path");
    assert_eq!(state.items[state.selected].value, "mobile-app");
    assert_eq!(app.store.view_state.scope, scope);
}

#[tokio::test]
async fn add_project_path_picker_defaults_to_current_directory() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();

    app.submit_add_project_path_picker(vec!["mobile-app".to_string()]);

    let Some(OverlayState::TextInput(state)) = &app.overlay else {
        panic!("expected path input");
    };
    assert_eq!(state.title, "Add project path");
    assert_eq!(state.prompt, "directory path for mobile-app:");
    assert_eq!(
        state.intent,
        TextIntent::AddProjectPath {
            project: "mobile-app".to_string()
        }
    );
    assert_eq!(
        state.input.text,
        std::env::current_dir().unwrap().display().to_string()
    );
}

#[tokio::test]
async fn invalid_project_path_keeps_input_for_retry() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing").display().to_string();

    app.submit_add_project_path("mobile-app".to_string(), missing.clone())
        .await
        .unwrap();

    assert_eq!(toast_severity(&app), Some(ToastSeverity::Error));
    assert!(toast_message(&app).unwrap().contains("could not resolve"));
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(TextInputState {
            intent: TextIntent::AddProjectPath { project },
            input,
            ..
        })) if project == "mobile-app" && input.text == missing
    ));
}

#[tokio::test]
async fn remove_project_path_requires_confirmation() {
    let mut app = test_app().await;

    app.submit_remove_project_path_value(
        "mobile-app".to_string(),
        vec!["/work/mobile".to_string()],
    );

    assert!(matches!(
        app.overlay,
        Some(OverlayState::Confirm(ConfirmState {
            intent: ConfirmIntent::RemoveProjectPath {
                ref project,
                ref path,
            },
            ..
        })) if project == "mobile-app" && path == "/work/mobile"
    ));
}

#[tokio::test]
async fn rename_project_submission_updates_selected_project() {
    let mut app = test_app().await;
    app.store
        .create_project("Agent Offload".to_string())
        .await
        .unwrap();

    app.execute(Action::BeginRenameProject).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(state)) if state.intent == PickerIntent::RenameProject
    ));
    assert!(app.notification.is_none());
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(app.overlay, Some(OverlayState::ProjectEdit(_))));
    app.submit_rename_project(crate::tui::overlay::ProjectEditState::new(
        "agent-offload".to_string(),
        "sideagent".to_string(),
        "AO".to_string(),
    ))
    .await
    .unwrap();

    assert_eq!(
        toast_message(&app).as_deref(),
        Some("renamed project sideagent prefix=AO · u undo")
    );
    assert!(
        app.store
            .projects
            .iter()
            .any(|project| project.key == "sideagent")
    );
}

#[tokio::test]
async fn rename_project_cancel_returns_to_browse() {
    let mut app = test_app().await;
    app.store
        .create_project("Agent Offload".to_string())
        .await
        .unwrap();
    let selected = app.list.selected_task();

    app.execute(Action::BeginRenameProject).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

    assert!(app.overlay.is_none());
    assert!(!app.view().detail_underlay);
    assert_eq!(app.list.selected_task(), selected);
}

#[tokio::test]
async fn delete_project_confirmation_removes_selected_project() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();

    app.execute(Action::BeginDeleteProject).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(state)) if state.intent == PickerIntent::DeleteProject
    ));
    assert!(app.notification.is_none());
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(
        app.overlay,
        Some(OverlayState::TextInput(TextInputState {
            intent: TextIntent::ConfirmDeleteProject { .. },
            ..
        }))
    ));
    app.submit_delete_project_name("mobile-app".to_string(), "mobile-app".to_string())
        .await
        .unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Confirm(ConfirmState {
            intent: ConfirmIntent::DeleteProject { .. },
            ..
        }))
    ));
    app.handle_overlay_key(key(KeyCode::Char('y')))
        .await
        .unwrap();

    assert_eq!(
        toast_message(&app).as_deref(),
        Some("deleted project mobile-app")
    );
    assert!(
        !app.store
            .projects
            .iter()
            .any(|project| project.key == "mobile-app")
    );
}

#[tokio::test]
async fn delete_project_cancel_discards_intent() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();
    app.list.focus_sidebar();
    let project_index = app
        .store
        .sidebar_entries
        .iter()
        .position(|entry| {
            entry.target
                == Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(
                    "mobile-app".to_string(),
                )))
        })
        .unwrap();
    app.list.select_sidebar(Some(project_index));

    app.execute(Action::BeginDeleteProject).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::TextInput(TextInputState {
            intent: TextIntent::ConfirmDeleteProject { .. },
            ..
        }))
    ));
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
}

#[tokio::test]
async fn project_prefix_editor_preserves_inputs_on_errors_and_saves_from_either_field() {
    use crate::tui::overlay::{ProjectEditField, handle_generic_overlay_paste};
    let mut app = test_app().await;
    app.store
        .create_project("Agent Offload".into())
        .await
        .unwrap();
    app.store.create_project("Other".into()).await.unwrap();
    app.begin_rename_project_for(Some("agent-offload"));
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    let Some(OverlayState::ProjectEdit(state)) = &app.overlay else {
        panic!("expected project editor")
    };
    assert_eq!(state.name.text, "Agent Offload");
    assert_eq!(state.prefix.text, "AO");
    assert_eq!(state.focus, ProjectEditField::Name);
    app.handle_overlay_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    app.overlay = Some(handle_generic_overlay_paste(
        "Renamed Project",
        app.overlay.take().unwrap(),
    ));
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    app.handle_overlay_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Char('x')))
        .await
        .unwrap();
    let Some(OverlayState::ProjectEdit(state)) = &app.overlay else {
        panic!("expected project editor")
    };
    assert_eq!(state.prefix.text, "X");
    // Submit from Name to prove the validation failure moves focus back to Prefix.
    app.handle_overlay_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT))
        .await
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    let Some(OverlayState::ProjectEdit(state)) = &app.overlay else {
        panic!("expected project editor")
    };
    assert_eq!(state.name.text, "Renamed Project");
    assert_eq!(state.prefix.text, "X");
    assert_eq!(state.focus, ProjectEditField::Prefix);
    assert!(
        toast_message(&app)
            .unwrap()
            .contains("invalid-project-prefix")
    );
    app.handle_overlay_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    app.overlay = Some(handle_generic_overlay_paste(
        "oth",
        app.overlay.take().unwrap(),
    ));
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    let Some(OverlayState::ProjectEdit(state)) = &app.overlay else {
        panic!("expected project editor")
    };
    assert_eq!(state.name.text, "Renamed Project");
    assert_eq!(state.prefix.text, "OTH");
    assert_eq!(state.focus, ProjectEditField::Prefix);
    assert!(
        toast_message(&app)
            .unwrap()
            .contains("project-prefix-exists")
    );
    assert!(
        app.store
            .projects
            .iter()
            .any(|p| p.key == "agent-offload" && p.prefix == "AO")
    );
    app.handle_overlay_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    app.overlay = Some(handle_generic_overlay_paste(
        "cus7",
        app.overlay.take().unwrap(),
    ));
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(app.overlay.is_none());
    assert!(
        app.store.projects.iter().any(|p| p.key == "renamed-project"
            && p.name == "Renamed Project"
            && p.prefix == "CUS7")
    );
    app.begin_rename_project_for(Some("renamed-project"));
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Char('2')))
        .await
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(app.overlay.is_none());
    assert!(
        app.store
            .projects
            .iter()
            .any(|p| p.key == "renamed-project2" && p.prefix == "CUS7")
    );
}
