use super::*;

async fn database() -> (tempfile::TempDir, Database, Workspace) {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::test_support::open_blank_database(&dir.path().join("projects.sqlite"))
        .await
        .unwrap();
    let workspace = db.list_workspaces().await.unwrap().remove(0);
    (dir, db, workspace)
}

async fn change_count(db: &Database) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM changes")
        .fetch_one(db.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn rename_preserves_generated_and_custom_prefixes_unless_explicit() {
    let (_dir, db, ws) = database().await;
    for (name, prefix, renamed) in [
        ("Agent Offload", None, "Sideagent"),
        ("Tools", Some(" x9 "), "Utilities"),
    ] {
        let original = db.create_project(&ws, name, prefix).await.unwrap().project;
        let outcome = db
            .rename_project(&ws, &original.key, renamed, None)
            .await
            .unwrap();
        assert_eq!(outcome.project.prefix, original.prefix);
        assert_eq!(outcome.project.id, original.id);
        let before = change_count(&db).await;
        let unchanged = db
            .rename_project(&ws, &outcome.project.key, renamed, Some(&original.prefix))
            .await
            .unwrap();
        assert!(!unchanged.changed);
        assert_eq!(change_count(&db).await, before);
    }
    let explicit = db
        .rename_project(&ws, "sideagent", "Sideagent", Some(" abc12345 "))
        .await
        .unwrap();
    assert_eq!(explicit.project.prefix, "ABC12345");
    let before = change_count(&db).await;
    for (prefix, code) in [
        ("X9", "project-prefix-exists"),
        ("éé", "invalid-project-prefix"),
    ] {
        let error = db
            .rename_project(&ws, "sideagent", "Wrong", Some(prefix))
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains(code));
    }
    let project = db
        .resolve_existing_project(&ws.id, "sideagent")
        .await
        .unwrap();
    assert_eq!(project.name, "Sideagent");
    assert_eq!(project.prefix, "ABC12345");
    assert_eq!(change_count(&db).await, before);
}

#[tokio::test]
async fn create_prefix_validation_uniqueness_and_active_project_contract() {
    let (_dir, db, ws) = database().await;
    let original = db
        .create_project(&ws, "Docs", Some(" d2 "))
        .await
        .unwrap()
        .project;
    assert_eq!(original.prefix, "D2");
    let before = change_count(&db).await;
    let unchanged = db.create_project(&ws, "Docs", Some(" d2 ")).await.unwrap();
    assert!(!unchanged.created);
    assert!(unchanged.change_id.is_none());
    assert_eq!(unchanged.project.id, original.id);
    for prefix in ["", "a", "abcdefghi", "a-b", "a b", "åb"] {
        for name in ["Docs", "Invalid"] {
            let error = db
                .create_project(&ws, name, Some(prefix))
                .await
                .err()
                .unwrap();
            assert!(
                error.to_string().contains("invalid-project-prefix"),
                "{error}"
            );
        }
    }
    let error = db
        .create_project(&ws, "Other", Some("d2"))
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("project-prefix-exists"));
    let error = db
        .create_project(&ws, "Docs", Some("NEW"))
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("project-exists"));
    assert!(error.to_string().contains("project rename"));
    assert_eq!(change_count(&db).await, before);
    assert!(
        db.resolve_existing_project(&ws.id, "invalid")
            .await
            .is_err()
    );
    let other_ws = db.create_workspace("Other").await.unwrap();
    assert_eq!(
        db.create_project(&other_ws, "Docs", Some("d2"))
            .await
            .unwrap()
            .project
            .prefix,
        "D2"
    );
    db.create_project(&ws, "Occupied", Some("BUSY"))
        .await
        .unwrap();
    let error = db
        .create_project(&ws, "Docs", Some("BUSY"))
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("project-exists"));
    assert!(error.to_string().contains("project rename"));
    assert_eq!(
        db.create_project(&ws, "Long", Some("abcdefgh"))
            .await
            .unwrap()
            .project
            .prefix,
        "ABCDEFGH"
    );
}

#[tokio::test]
async fn restore_prefix_excludes_self_and_rejects_collisions_atomically() {
    let (_dir, db, ws) = database().await;
    let project = db
        .create_project(&ws, "Docs", Some("DOC"))
        .await
        .unwrap()
        .project;
    // Retained projects reserve their prefixes even while deleted.
    sqlx::query("UPDATE projects SET deleted = 1 WHERE id = ?")
        .bind(&project.id)
        .execute(db.pool())
        .await
        .unwrap();
    db.create_project(&ws, "Other", Some("BUSY")).await.unwrap();
    let before = change_count(&db).await;
    let error = db
        .create_project(&ws, "Docs", Some("busy"))
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("project-prefix-exists"));
    assert_eq!(change_count(&db).await, before);
    assert!(db.resolve_existing_project(&ws.id, "docs").await.is_err());
    let error = db
        .create_project(&ws, "Third", Some("doc"))
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("project-prefix-exists"));
    for prefix in [Some(" doc "), Some("CUSTOM"), None] {
        let outcome = db.create_project(&ws, "Docs", prefix).await.unwrap();
        assert!(outcome.created);
        assert_eq!(outcome.project.id, project.id);
        let expected = if prefix == Some(" doc ") {
            "DOC"
        } else {
            "CUSTOM"
        };
        assert_eq!(outcome.project.prefix, expected);
        let payload: String = sqlx::query_scalar("SELECT payload FROM changes WHERE change_id = ?")
            .bind(outcome.change_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(payload["prefix"], expected);
        sqlx::query("UPDATE projects SET deleted = 1 WHERE id = ?")
            .bind(&project.id)
            .execute(db.pool())
            .await
            .unwrap();
    }
    sqlx::query("UPDATE projects SET prefix = id WHERE id = ?")
        .bind(&project.id)
        .execute(db.pool())
        .await
        .unwrap();
    let restored = db.create_project(&ws, "Docs", None).await.unwrap();
    assert_eq!(restored.project.prefix, "DCS");
}
