use super::*;

async fn set_marker(store: &TuiStore, value: &str) {
    let mut conn = aven_core::test_support::acquire(&store.database)
        .await
        .unwrap();
    aven_core::test_support::set_meta(&mut conn, "tui_onboarding_version", value)
        .await
        .unwrap();
}

#[tokio::test]
async fn fresh_database_is_due_until_completed() {
    let store = test_store().await;
    assert_eq!(
        store.onboarding_status().await.unwrap(),
        OnboardingStatus::Due
    );

    store.complete_onboarding().await.unwrap();

    assert_eq!(
        store.onboarding_status().await.unwrap(),
        OnboardingStatus::Complete
    );
}

#[tokio::test]
async fn established_database_is_not_treated_as_first_launch() {
    let mut store = test_store().await;
    create_selected_task(&mut store, "Existing task").await;

    assert_eq!(
        store.onboarding_status().await.unwrap(),
        OnboardingStatus::Established
    );
}

#[tokio::test]
async fn marker_parsing_is_trimmed_and_downgrade_safe() {
    let store = test_store().await;
    for value in [" 1 ", "2", "4294967295"] {
        set_marker(&store, value).await;
        assert_eq!(
            store.onboarding_status().await.unwrap(),
            OnboardingStatus::Complete,
            "marker {value:?}"
        );
    }
}

#[tokio::test]
async fn invalid_or_older_markers_are_due_for_empty_database() {
    let store = test_store().await;
    for value in ["0", "-1", "invalid", "4294967296"] {
        set_marker(&store, value).await;
        assert_eq!(
            store.onboarding_status().await.unwrap(),
            OnboardingStatus::Due,
            "marker {value:?}"
        );
    }
}

#[tokio::test]
async fn completion_preserves_future_marker() {
    let store = test_store().await;
    set_marker(&store, "2").await;

    store.complete_onboarding().await.unwrap();

    let mut conn = aven_core::test_support::acquire(&store.database)
        .await
        .unwrap();
    assert_eq!(
        aven_core::test_support::get_meta(&mut conn, "tui_onboarding_version")
            .await
            .unwrap()
            .as_deref(),
        Some("2")
    );
}
