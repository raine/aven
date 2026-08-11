use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::{
    AppConfig, CustomTuiCommandConfig, CustomTuiCommandExecution, CustomTuiCommandSuccess,
    CustomTuiCommandTarget,
};

use super::*;

fn config(program: &str, success: CustomTuiCommandSuccess) -> AppConfig {
    let mut config = AppConfig::default();
    config.tui.commands.push(CustomTuiCommandConfig {
        name: "dispatch".to_string(),
        aliases: vec!["custom-dispatch".to_string()],
        description: "dispatch selected task".to_string(),
        program: PathBuf::from(program),
        cwd: None,
        env: Default::default(),
        timeout_seconds: None,
        args: (program == "/usr/bin/tee")
            .then(|| "/dev/null".to_string())
            .into_iter()
            .collect(),
        keys: vec![],
        detail_keys: None,
        target: CustomTuiCommandTarget::None,
        execution: CustomTuiCommandExecution::Wait,
        on_success: success,
    });
    config
}

fn capture_config(
    path: &std::path::Path,
    target: CustomTuiCommandTarget,
    keys: Vec<String>,
) -> AppConfig {
    let mut config = config("/usr/bin/tee", CustomTuiCommandSuccess::Stay);
    let command = &mut config.tui.commands[0];
    command.args = vec![path.to_string_lossy().into_owned()];
    command.target = target;
    command.keys = keys;
    config
}

fn captured_json(path: &std::path::Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

async fn poll_until_complete(app: &mut App) {
    for _ in 0..100 {
        if app.poll_custom_commands().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("custom command did not complete");
}

async fn external_database(app: &App) -> aven_core::db::Database {
    let path = app
        ._test_database_dir
        .as_ref()
        .expect("test database directory")
        .path()
        .join("test.db");
    aven_core::db::Database::open(&path).await.unwrap()
}

fn add_command(
    config: &mut AppConfig,
    name: &str,
    program: &Path,
    args: &[&str],
    success: CustomTuiCommandSuccess,
) {
    config.tui.commands.push(CustomTuiCommandConfig {
        name: name.to_string(),
        aliases: vec![],
        description: format!("run {name}"),
        program: program.to_path_buf(),
        cwd: None,
        env: Default::default(),
        timeout_seconds: None,
        args: args.iter().map(|arg| (*arg).to_string()).collect(),
        keys: vec![],
        detail_keys: None,
        target: CustomTuiCommandTarget::None,
        execution: CustomTuiCommandExecution::Wait,
        on_success: success,
    });
}

fn compile_process_fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/custom_command_process.rs");
    let executable = dir.path().join("custom-command-process");
    let status = std::process::Command::new("rustc")
        .args(["--edition=2024", "-o"])
        .arg(&executable)
        .arg(source)
        .status()
        .unwrap();
    assert!(status.success());
    (dir, executable)
}

#[tokio::test]
async fn app_projects_active_invocation_paths_and_excludes_environment_values() {
    let mut app = test_app().await;
    let capture_dir = tempfile::tempdir().unwrap();
    let capture = capture_dir.path().join("context.json");
    let mut app_config = capture_config(&capture, CustomTuiCommandTarget::None, vec![]);
    app_config.tui.commands[0]
        .env
        .insert("ACCESS_TOKEN".to_string(), "secret-marker".to_string());
    app.set_config(app_config);

    app.execute_custom_command(0, "dispatch").await.unwrap();
    poll_until_complete(&mut app).await;

    let json = captured_json(&capture);
    let invocation = &json["invocation"];
    assert_eq!(invocation["tui_pid"], std::process::id());
    assert_eq!(
        invocation["origin_cwd"],
        app.custom_command_planning.origin_cwd.to_str().unwrap()
    );
    assert_eq!(
        invocation["cwd"],
        app.custom_command_planning.origin_cwd.to_str().unwrap()
    );
    for (field, path) in [
        ("aven_exe", app.custom_command_planning.aven_exe.as_ref()),
        (
            "config_dir",
            app.custom_command_planning.config_dir.as_ref(),
        ),
        ("db_path", app.custom_command_planning.db_path.as_ref()),
        ("blob_dir", app.custom_command_planning.blob_dir.as_ref()),
    ] {
        let path = path.expect("active application path");
        let expected = if path.is_absolute() {
            path.clone()
        } else {
            app.custom_command_planning.origin_cwd.join(path)
        };
        assert_eq!(invocation[field], expected.to_str().unwrap(), "{field}");
    }
    assert!(
        !std::fs::read_to_string(capture)
            .unwrap()
            .contains("secret-marker")
    );
}

#[tokio::test]
async fn configured_keybinding_executes_custom_command() {
    let mut app = test_app().await;
    let mut app_config = config("/usr/bin/tee", CustomTuiCommandSuccess::Stay);
    app_config.tui.commands[0].keys = vec!["z d".to_string()];
    app.set_config(app_config);

    app.handle_normal_key(KeyCode::Char('z')).await.unwrap();
    assert!(!app.pending_shortcut.is_empty());
    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
    assert!(app.pending_shortcut.is_empty());
    assert!(toast_message(&app).unwrap().contains("running :dispatch"));

    poll_until_complete(&mut app).await;
    assert!(toast_message(&app).unwrap().contains("completed"));
}

#[tokio::test]
async fn detail_keybinding_override_executes_custom_command() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail task")).await;
    let mut app_config = config("/usr/bin/tee", CustomTuiCommandSuccess::Stay);
    app_config.tui.commands[0].keys = vec!["z d".to_string()];
    app_config.tui.commands[0].detail_keys = Some(vec!["Z".to_string()]);
    app.set_config(app_config);
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Char('Z')), (80, 24).into())
        .await
        .unwrap();
    assert!(toast_message(&app).unwrap().contains("running :dispatch"));

    poll_until_complete(&mut app).await;
    assert!(app.detail.is_active());
    assert!(toast_message(&app).unwrap().contains("completed"));
}

#[tokio::test]
async fn successful_waiting_command_applies_quit_policy() {
    let mut app = test_app().await;
    app.set_config(config("/usr/bin/tee", CustomTuiCommandSuccess::Quit));

    app.execute_custom_command(0, "custom-dispatch")
        .await
        .unwrap();
    assert!(!app.should_quit);
    poll_until_complete(&mut app).await;

    assert!(app.should_quit);
}

#[tokio::test]
async fn failed_waiting_command_leaves_app_open_with_error() {
    let mut app = test_app().await;
    app.set_config(config("/usr/bin/false", CustomTuiCommandSuccess::Quit));

    app.execute_custom_command(0, "dispatch").await.unwrap();
    poll_until_complete(&mut app).await;

    assert!(!app.should_quit);
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Error));
    assert!(toast_message(&app).unwrap().contains(":dispatch"));
}

#[tokio::test]
async fn successful_stay_policy_leaves_app_open() {
    let mut app = test_app().await;
    app.set_config(config("/usr/bin/tee", CustomTuiCommandSuccess::Stay));

    app.execute_custom_command(0, "dispatch").await.unwrap();
    poll_until_complete(&mut app).await;

    assert!(!app.should_quit);
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Info));
    assert!(toast_message(&app).unwrap().contains("completed"));
}

#[tokio::test]
async fn stay_policy_retains_the_current_projection_after_external_mutation() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Before stay")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    app.set_config(config("/usr/bin/tee", CustomTuiCommandSuccess::Stay));

    app.execute_custom_command(0, "dispatch").await.unwrap();
    external_database(&app)
        .await
        .update_task(
            &app.store.active_workspace,
            &task_id,
            crate::operations::TaskUpdate {
                title: Some("After stay".to_string()),
                ..crate::operations::TaskUpdate::default()
            },
        )
        .await
        .unwrap();
    poll_until_complete(&mut app).await;

    assert_eq!(app.store.tasks[selected].task.title, "Before stay");
    assert!(!app.should_quit);
}

#[tokio::test]
async fn refresh_policy_updates_projection_and_preserves_navigation_identity() {
    let mut app = test_app().await;
    let mut first_draft = test_task_draft("Alpha");
    first_draft.priority = "high".to_string();
    let first = create_and_select_task(&mut app, first_draft).await;
    let first_id = app.store.tasks[first].task.id.clone();
    let mut selected_draft = test_task_draft("Zulu");
    selected_draft.priority = "high".to_string();
    let selected = create_and_select_task(&mut app, selected_draft).await;
    let selected_id = app.store.tasks[selected].task.id.clone();
    app.store.view_state.view = TaskView::Inbox;
    app.store.view_state.order = TaskOrder::Title;
    app.store.view_state.filter_modifiers.priority = Some("high".to_string());
    app.refresh().await.unwrap();
    let selected = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == selected_id)
        .unwrap();
    app.list.select_task(Some(selected));
    app.list.mark(first_id.clone());
    app.list.mark(selected_id.clone());
    app.show_detail(0);
    let view_state = app.store.view_state.clone();
    app.set_config(config("/usr/bin/tee", CustomTuiCommandSuccess::Refresh));

    app.execute_custom_command(0, "dispatch").await.unwrap();
    external_database(&app)
        .await
        .update_task(
            &app.store.active_workspace,
            &selected_id,
            crate::operations::TaskUpdate {
                title: Some("Aardvark".to_string()),
                ..crate::operations::TaskUpdate::default()
            },
        )
        .await
        .unwrap();
    poll_until_complete(&mut app).await;

    let selected = app.store.selected_task(app.list.selected_task()).unwrap();
    assert_eq!(selected.task.id, selected_id);
    assert_eq!(selected.task.title, "Aardvark");
    assert!(app.detail.is_active());
    assert_eq!(app.store.view_state, view_state);
    assert!(app.list.marked_task_ids().contains(&first_id));
    assert!(app.list.marked_task_ids().contains(&selected_id));
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Info));
}

#[tokio::test]
async fn refresh_policy_reconciles_a_task_that_leaves_the_active_view() {
    let mut app = test_app().await;
    let disappearing = create_and_select_task(&mut app, test_task_draft("Leaves inbox")).await;
    let disappearing_id = app.store.tasks[disappearing].task.id.clone();
    create_and_select_task(&mut app, test_task_draft("Remains in inbox")).await;
    app.store.view_state.view = TaskView::Inbox;
    app.refresh().await.unwrap();
    let disappearing = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == disappearing_id)
        .unwrap();
    app.list.select_task(Some(disappearing));
    app.list.mark(disappearing_id.clone());
    app.set_config(config("/usr/bin/tee", CustomTuiCommandSuccess::Refresh));

    app.execute_custom_command(0, "dispatch").await.unwrap();
    external_database(&app)
        .await
        .update_task(
            &app.store.active_workspace,
            &disappearing_id,
            crate::operations::TaskUpdate {
                status: Some("done".to_string()),
                ..crate::operations::TaskUpdate::default()
            },
        )
        .await
        .unwrap();
    poll_until_complete(&mut app).await;

    assert!(
        app.store
            .tasks
            .iter()
            .all(|item| item.task.id != disappearing_id)
    );
    assert!(!app.list.marked_task_ids().contains(&disappearing_id));
    assert!(app.store.selected_task(app.list.selected_task()).is_some());
}

#[tokio::test]
async fn refresh_failure_leaves_app_open_with_bounded_useful_error() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Before failure")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    app.set_config(config("/usr/bin/tee", CustomTuiCommandSuccess::Refresh));

    app.execute_custom_command(0, "dispatch").await.unwrap();
    external_database(&app)
        .await
        .update_task(
            &app.store.active_workspace,
            &task_id,
            crate::operations::TaskUpdate {
                title: Some("After failure".to_string()),
                ..crate::operations::TaskUpdate::default()
            },
        )
        .await
        .unwrap();
    app.store.fail_next_refresh();
    poll_until_complete(&mut app).await;

    assert!(!app.should_quit);
    assert_eq!(app.store.tasks[selected].task.title, "Before failure");
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Error));
    let message = toast_message(&app).unwrap();
    assert!(
        message.starts_with(":dispatch completed, but Aven could not refresh: ")
            && message.contains("injected refresh failure"),
        "{message}"
    );
}

#[tokio::test]
async fn refresh_and_quit_requires_a_successful_refresh() {
    let mut successful = test_app().await;
    successful.set_config(config(
        "/usr/bin/tee",
        CustomTuiCommandSuccess::RefreshAndQuit,
    ));
    successful
        .execute_custom_command(0, "dispatch")
        .await
        .unwrap();
    poll_until_complete(&mut successful).await;
    assert!(successful.should_quit);

    let mut failed = test_app().await;
    failed.set_config(config(
        "/usr/bin/tee",
        CustomTuiCommandSuccess::RefreshAndQuit,
    ));
    failed.execute_custom_command(0, "dispatch").await.unwrap();
    failed.store.fail_next_refresh();
    poll_until_complete(&mut failed).await;
    assert!(!failed.should_quit);
    assert_eq!(toast_severity(&failed), Some(ToastSeverity::Error));
}

#[tokio::test]
async fn nonzero_exit_never_refreshes_or_quits() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Before nonzero")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    app.set_config(config(
        "/usr/bin/false",
        CustomTuiCommandSuccess::RefreshAndQuit,
    ));

    app.execute_custom_command(0, "dispatch").await.unwrap();
    external_database(&app)
        .await
        .update_task(
            &app.store.active_workspace,
            &task_id,
            crate::operations::TaskUpdate {
                title: Some("After nonzero".to_string()),
                ..crate::operations::TaskUpdate::default()
            },
        )
        .await
        .unwrap();
    poll_until_complete(&mut app).await;

    assert!(!app.should_quit);
    assert_eq!(app.store.tasks[selected].task.title, "Before nonzero");
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Error));
    assert!(toast_message(&app).unwrap().contains("exited with status"));
}

#[tokio::test]
async fn out_of_order_completions_apply_effects_and_preserve_error_notification() {
    let mut app = test_app().await;
    let selected =
        create_and_select_task(&mut app, test_task_draft("Before ordered effects")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    let (_fixture_dir, fixture) = compile_process_fixture();
    let mut app_config = AppConfig::default();
    add_command(
        &mut app_config,
        "slow-stay",
        &fixture,
        &["delayed-exit", "600", "0"],
        CustomTuiCommandSuccess::Stay,
    );
    add_command(
        &mut app_config,
        "medium-refresh",
        &fixture,
        &["delayed-exit", "300", "0"],
        CustomTuiCommandSuccess::Refresh,
    );
    add_command(
        &mut app_config,
        "fast-failure",
        &fixture,
        &["delayed-exit", "50", "9", "fixture failure"],
        CustomTuiCommandSuccess::RefreshAndQuit,
    );
    app.set_config(app_config);

    external_database(&app)
        .await
        .update_task(
            &app.store.active_workspace,
            &task_id,
            crate::operations::TaskUpdate {
                title: Some("After ordered effects".to_string()),
                ..crate::operations::TaskUpdate::default()
            },
        )
        .await
        .unwrap();
    app.execute_custom_command(0, "slow-stay").await.unwrap();
    app.execute_custom_command(1, "medium-refresh")
        .await
        .unwrap();
    app.execute_custom_command(2, "fast-failure").await.unwrap();

    let failure = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if app.poll_custom_commands().await
                && toast_message(&app).is_some_and(|message| message.contains("fast-failure"))
            {
                break toast_message(&app).unwrap();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fast command did not complete");
    assert!(!app.should_quit);
    assert_eq!(
        app.store.tasks[selected].task.title,
        "Before ordered effects"
    );

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            app.poll_custom_commands().await;
            if app.store.tasks[selected].task.title == "After ordered effects" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("refresh command did not complete");
    assert_eq!(toast_message(&app).as_deref(), Some(failure.as_str()));

    tokio::time::timeout(Duration::from_secs(2), async {
        while app.custom_commands.work_pending() {
            app.poll_custom_commands().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("stay command did not complete");
    assert_eq!(toast_message(&app).as_deref(), Some(failure.as_str()));
    assert!(!app.should_quit);
}

#[test]
fn refresh_errors_are_sanitized_and_bounded() {
    let reason = format!("{}\n\u{1b}[31m", "x".repeat(600));
    let error = anyhow::anyhow!(reason);
    let message = crate::tui::app_custom_commands::refresh_error_message("dispatch", &error);
    let rendered_reason = message.split_once(": ").unwrap().1;

    assert!(
        rendered_reason.chars().count()
            <= crate::tui::app_custom_commands::REFRESH_ERROR_CHAR_LIMIT
    );
    assert!(!rendered_reason.contains('\n'));
    assert!(!rendered_reason.contains('\u{1b}'));
    assert!(rendered_reason.ends_with('…'));
}

#[tokio::test]
async fn target_none_is_available_without_primary_in_list_and_detail_contexts() {
    let dir = tempfile::tempdir().unwrap();
    let list_path = dir.path().join("list.json");
    let detail_path = dir.path().join("detail.json");
    let mut app = test_app().await;
    app.list.select_task(None);
    app.set_config(capture_config(
        &list_path,
        CustomTuiCommandTarget::None,
        vec![],
    ));

    app.execute_custom_command(0, "dispatch").await.unwrap();
    poll_until_complete(&mut app).await;
    let list = captured_json(&list_path);
    assert!(list["selection"]["primary"].is_null());
    assert_eq!(list["targeting"]["targets"], serde_json::json!([]));

    create_and_select_task(&mut app, test_task_draft("detail primary")).await;
    app.show_detail(0);
    app.set_config(capture_config(
        &detail_path,
        CustomTuiCommandTarget::None,
        vec![],
    ));
    app.execute_custom_command(0, "dispatch").await.unwrap();
    poll_until_complete(&mut app).await;
    let detail = captured_json(&detail_path);
    assert!(detail["selection"]["primary"].is_object());
    assert_eq!(detail["targeting"]["resolved_from"], "none");
}

#[tokio::test]
async fn focused_target_uses_list_or_displayed_detail_primary_and_requires_one() {
    let dir = tempfile::tempdir().unwrap();
    let list_path = dir.path().join("list.json");
    let detail_path = dir.path().join("detail.json");
    let mut app = test_app().await;
    app.list.select_task(None);
    app.set_config(capture_config(
        &list_path,
        CustomTuiCommandTarget::Focused,
        vec![],
    ));

    app.execute_custom_command(0, "dispatch").await.unwrap();
    assert_eq!(
        toast_message(&app).as_deref(),
        Some(":dispatch is disabled: requires a focused task")
    );
    assert!(!list_path.exists());

    let selected = create_and_select_task(&mut app, test_task_draft("displayed primary")).await;
    let primary_id = app.store.tasks[selected].task.id.clone();
    app.execute_custom_command(0, "dispatch").await.unwrap();
    poll_until_complete(&mut app).await;
    assert_eq!(
        captured_json(&list_path)["targeting"]["targets"][0]["id"],
        primary_id.as_str()
    );

    app.show_detail(0);
    app.detail
        .state_mut()
        .unwrap()
        .set_focused_target(Some(DetailTargetId::Task {
            section: DetailSection::EpicChildren,
            task_id: crate::test_support::task_id("related-focus"),
        }));
    app.set_config(capture_config(
        &detail_path,
        CustomTuiCommandTarget::Focused,
        vec![],
    ));
    app.execute_custom_command(0, "dispatch").await.unwrap();
    poll_until_complete(&mut app).await;
    assert_eq!(
        captured_json(&detail_path)["targeting"]["targets"][0]["id"],
        primary_id.as_str()
    );
}

#[tokio::test]
async fn marked_target_requires_marks_and_preserves_visible_order_in_list_and_detail() {
    let dir = tempfile::tempdir().unwrap();
    let list_path = dir.path().join("list.json");
    let detail_path = dir.path().join("detail.json");
    let mut app = test_app().await;
    let first = create_and_select_task(&mut app, test_task_draft("first mark")).await;
    let first_id = app.store.tasks[first].task.id.clone();
    let second = create_and_select_task(&mut app, test_task_draft("second mark")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    app.set_config(capture_config(
        &list_path,
        CustomTuiCommandTarget::Marked,
        vec![],
    ));

    app.execute_custom_command(0, "dispatch").await.unwrap();
    assert_eq!(
        toast_message(&app).as_deref(),
        Some(":dispatch is disabled: requires one or more marked tasks")
    );

    app.list.mark(second_id);
    app.execute_custom_command(0, "dispatch").await.unwrap();
    poll_until_complete(&mut app).await;
    let one = captured_json(&list_path);
    assert_eq!(one["targeting"]["targets"].as_array().unwrap().len(), 1);

    app.list.mark(first_id);
    let expected = app
        .store
        .tasks
        .iter()
        .filter(|item| app.list.marked_task_ids().contains(&item.task.id))
        .map(|item| item.task.id.to_string())
        .collect::<Vec<_>>();
    app.set_config(capture_config(
        &detail_path,
        CustomTuiCommandTarget::Marked,
        vec![],
    ));
    app.show_detail(0);
    app.begin_command().await;
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.marked_task_count == 0 && state.custom_command_marked_task_count == 2
    ));
    app.overlay = None;
    app.execute_custom_command(0, "dispatch").await.unwrap();
    poll_until_complete(&mut app).await;
    let detail = captured_json(&detail_path);
    let actual = detail["targeting"]["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|target| target["id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
    assert_eq!(detail["selection"]["marked"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn marked_or_focused_falls_back_then_prefers_marks() {
    let dir = tempfile::tempdir().unwrap();
    let fallback_path = dir.path().join("fallback.json");
    let marked_path = dir.path().join("marked.json");
    let mut app = test_app().await;
    let primary = create_and_select_task(&mut app, test_task_draft("primary")).await;
    let primary_id = app.store.tasks[primary].task.id.clone();
    let marked = create_and_select_task(&mut app, test_task_draft("marked")).await;
    let marked_id = app.store.tasks[marked].task.id.clone();
    let primary = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == primary_id)
        .unwrap();
    app.list.select_task(Some(primary));
    app.set_config(capture_config(
        &fallback_path,
        CustomTuiCommandTarget::MarkedOrFocused,
        vec![],
    ));

    app.execute_custom_command(0, "dispatch").await.unwrap();
    poll_until_complete(&mut app).await;
    let fallback = captured_json(&fallback_path);
    assert_eq!(fallback["targeting"]["resolved_from"], "focused");
    assert_eq!(
        fallback["targeting"]["targets"][0]["id"],
        primary_id.as_str()
    );

    app.list.mark(marked_id.clone());
    app.set_config(capture_config(
        &marked_path,
        CustomTuiCommandTarget::MarkedOrFocused,
        vec![],
    ));
    app.execute_custom_command(0, "dispatch").await.unwrap();
    poll_until_complete(&mut app).await;
    let preferred = captured_json(&marked_path);
    assert_eq!(preferred["targeting"]["resolved_from"], "marked");
    assert_eq!(
        preferred["targeting"]["targets"][0]["id"],
        marked_id.as_str()
    );
}

#[tokio::test]
async fn palette_and_shortcut_invocations_produce_identical_targets() {
    let dir = tempfile::tempdir().unwrap();
    let palette_path = dir.path().join("palette.json");
    let shortcut_path = dir.path().join("shortcut.json");
    let mut app = test_app().await;
    let first = create_and_select_task(&mut app, test_task_draft("first")).await;
    let first_id = app.store.tasks[first].task.id.clone();
    let second = create_and_select_task(&mut app, test_task_draft("second")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    app.list.mark(first_id);
    app.list.mark(second_id);
    app.set_config(capture_config(
        &palette_path,
        CustomTuiCommandTarget::MarkedOrFocused,
        vec!["z d".to_string()],
    ));

    app.begin_command().await;
    for ch in "dispatch".chars() {
        app.handle_overlay_key(key(KeyCode::Char(ch)))
            .await
            .unwrap();
    }
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    poll_until_complete(&mut app).await;

    app.set_config(capture_config(
        &shortcut_path,
        CustomTuiCommandTarget::MarkedOrFocused,
        vec!["z d".to_string()],
    ));
    app.handle_normal_key(KeyCode::Char('z')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
    poll_until_complete(&mut app).await;

    let palette = captured_json(&palette_path);
    let shortcut = captured_json(&shortcut_path);
    assert_eq!(palette["targeting"], shortcut["targeting"]);
    assert_eq!(palette["selection"], shortcut["selection"]);
}
