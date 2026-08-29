use super::*;

#[tokio::test]
async fn constructor_applies_supplied_configuration_to_every_owner() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let pool = crate::test_support::open_db(&db_path).await.unwrap();
    reset_default_workspace(&pool).await;
    let database = aven_core::db::Database::open(&db_path).await.unwrap();
    let mut config = AppConfig::default();
    config.tui.columns = vec![crate::config::TaskColumnConfig {
        name: "Configured".to_string(),
        statuses: vec![
            "inbox".to_string(),
            "backlog".to_string(),
            "todo".to_string(),
            "active".to_string(),
            "done".to_string(),
            "canceled".to_string(),
        ],
    }];
    config.local.inline_images = crate::config::InlineImagesConfig::Off;
    config.agent.task_intake.command = Some("configured-intake".to_string());
    config.sync.enabled = true;

    let app = App::new_with_view_state_and_config(
        database,
        crate::workspaces::Workspace::default(),
        TaskViewState::default(),
        config,
    )
    .await
    .unwrap();

    assert_eq!(app.store.task_columns()[0].name, "Configured");
    assert_eq!(
        app.store.config().agent.task_intake.command.as_deref(),
        Some("configured-intake")
    );
    assert!(app.store.config().sync.enabled);
    assert_eq!(
        app.store.config().local.inline_images,
        crate::config::InlineImagesConfig::Off
    );
    assert_eq!(
        app.intake.config().agent.task_intake.command.as_deref(),
        Some("configured-intake")
    );
    assert_eq!(
        app.intake.config().local.inline_images,
        crate::config::InlineImagesConfig::Off
    );
    assert_eq!(app.inline_image_backend, InlineImageBackend::None);
}
