use super::*;
use crate::ids::WorkspaceId;
use crate::query::test_support::*;
use sqlx::SqliteConnection;

#[test]
fn score_text_lane_does_not_normalize_ref_glyphs() {
    assert_eq!(score_contiguous_text_lane("looking glass", "100king"), None);
    assert_eq!(score_contiguous_text_lane("looking glass", "100k1ng"), None);
    assert!(score_contiguous_text_lane("looking glass", "glass").is_some());
    assert!(score_contiguous_text_lane("looking glass", "looking").is_some());
}

#[test]
fn score_text_lane_matches_parser_owned_quoted_phrase() {
    let parsed = parser::parse_task_search_query("\"pager rotation\"");
    let (_, span) = score_text_lane("contains pager rotation context", &parsed).unwrap();

    assert_eq!(&"contains pager rotation context"[span], "pager rotation");
    assert!(score_text_lane("contains pager context", &parsed).is_none());
}

#[test]
fn score_text_lane_handles_unsafe_parser_input_without_panic() {
    for input in ["\"", "\"(", "a*b", "\"unfinished", "x OR y", "\"*\""] {
        let parsed = parser::parse_task_search_query(input);
        let _ = score_text_lane("any task body", &parsed);
    }
}

#[tokio::test]
async fn task_search_finds_done_labels_and_notes() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;
    insert_test_task(
        &mut conn,
        "7KQ9A1X4MV2P8D6R",
        "Done release cleanup",
        "done",
        "high",
        "001",
    )
    .await;
    insert_test_task(
        &mut conn,
        "8KQ9A1X4MV2P8D6R",
        "Plain inbox",
        "inbox",
        "none",
        "002",
    )
    .await;
    insert_test_task(
        &mut conn,
        "9KQ9A1X4MV2P8D6R",
        "Add iOS API auth flow",
        "todo",
        "medium",
        "003",
    )
    .await;
    insert_test_label(&mut conn, "8KQ9A1X4MV2P8D6R", "security").await;
    sqlx::query(
        "UPDATE tasks SET description = 'schedule pager rotation handoff' WHERE id = '9KQ9A1X4MV2P8D6R'",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO notes(id, task_id, body, created_at, change_id)
         VALUES ('note-search', '7KQ9A1X4MV2P8D6R', 'contains pager rotation context', '003', 'change-search')",
    )
    .execute(&mut *conn)
    .await
    .unwrap();

    let done = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "release cleanup".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(listed_titles_from_search(&done), ["Done release cleanup"]);
    assert_eq!(done[0].matched_field, SearchMatchedField::Title);

    let label = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "security".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(listed_titles_from_search(&label), ["Plain inbox"]);
    assert_eq!(label[0].matched_field, SearchMatchedField::Label);

    let title_tokens = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "ios auth".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        listed_titles_from_search(&title_tokens),
        ["Add iOS API auth flow"]
    );
    assert_eq!(title_tokens[0].matched_field, SearchMatchedField::Title);

    let note = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "pager rotation".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();
    let note_titles = listed_titles_from_search(&note);
    assert!(note_titles.contains(&"Done release cleanup"));
    assert!(note_titles.contains(&"Add iOS API auth flow"));
    let note_match = note
        .iter()
        .find(|r| r.item.task.title == "Done release cleanup")
        .expect("note match should be in results");
    assert_eq!(note_match.matched_field, SearchMatchedField::Note);
    assert!(
        note_match
            .snippet
            .as_deref()
            .is_some_and(|value| value.contains("pager rotation"))
    );

    let quoted_note = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "\"pager rotation\"".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();
    let quoted_note_titles = listed_titles_from_search(&quoted_note);
    assert!(quoted_note_titles.contains(&"Done release cleanup"));
    assert!(quoted_note_titles.contains(&"Add iOS API auth flow"));
    let quoted_note_match = quoted_note
        .iter()
        .find(|r| r.item.task.title == "Done release cleanup")
        .expect("note match should be in results");
    assert_eq!(quoted_note_match.matched_field, SearchMatchedField::Note);
    assert!(
        quoted_note_match
            .snippet
            .as_deref()
            .is_some_and(|value| value.contains("pager rotation"))
    );

    let desc_phrase = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "\"pager rotation handoff\"".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        listed_titles_from_search(&desc_phrase),
        ["Add iOS API auth flow"]
    );
    assert_eq!(
        desc_phrase[0].matched_field,
        SearchMatchedField::Description
    );
    assert!(
        desc_phrase[0]
            .snippet
            .as_deref()
            .is_some_and(|value| value.contains("pager rotation handoff"))
    );
}

fn task_search_fts_match(workspace_id: &WorkspaceId, term: &str) -> String {
    fn phrase(value: &str) -> String {
        format!("\"{}\"", value.replace('"', "\"\""))
    }

    format!(
        "workspace_token:{} {}",
        phrase(workspace_id.as_str()),
        phrase(term)
    )
}

async fn task_search_fts_match_count(conn: &mut SqliteConnection, expression: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM task_search_fts WHERE task_search_fts MATCH ?",
    )
    .bind(expression)
    .fetch_one(&mut *conn)
    .await
    .unwrap()
}

#[tokio::test]
async fn task_search_document_update_removes_stale_fts_terms() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;
    insert_test_task(
        &mut conn,
        "7KQ9A1X4MV2P8D6R",
        "FTS maintenance probe",
        "todo",
        "none",
        "001",
    )
    .await;

    let workspace_id = crate::workspaces::default_workspace_id();
    sqlx::query(
        "UPDATE task_search_documents SET notes = 'obsolete pager phrase'
         WHERE workspace_id = ? AND task_id = ?",
    )
    .bind(&workspace_id)
    .bind("7KQ9A1X4MV2P8D6R")
    .execute(&mut *conn)
    .await
    .unwrap();

    let obsolete = task_search_fts_match(&workspace_id, "obsolete pager phrase");
    assert_eq!(task_search_fts_match_count(&mut conn, &obsolete).await, 1);

    sqlx::query(
        "UPDATE task_search_documents SET notes = 'fresh pager phrase'
         WHERE workspace_id = ? AND task_id = ?",
    )
    .bind(&workspace_id)
    .bind("7KQ9A1X4MV2P8D6R")
    .execute(&mut *conn)
    .await
    .unwrap();

    assert_eq!(task_search_fts_match_count(&mut conn, &obsolete).await, 0);

    let fresh = task_search_fts_match(&workspace_id, "fresh pager phrase");
    assert_eq!(task_search_fts_match_count(&mut conn, &fresh).await, 1);
}

#[tokio::test]
async fn task_search_requires_contiguous_text_matches() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;
    insert_test_task(
        &mut conn,
        "7KQ9A1X4MV2P8D6R",
        "Fix dashboard timers resetting spuriously for idle agents",
        "done",
        "medium",
        "001",
    )
    .await;
    insert_test_task(
        &mut conn,
        "8KQ9A1X4MV2P8D6R",
        "Automate multi-shell testing for shell-sensitive test cases",
        "done",
        "medium",
        "002",
    )
    .await;

    let items = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "testing".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();

    assert_eq!(
        listed_titles_from_search(&items),
        ["Automate multi-shell testing for shell-sensitive test cases"]
    );
    assert_eq!(items[0].matched_field, SearchMatchedField::Title);
}

#[tokio::test]
async fn task_search_ranks_refs_and_controls_deleted_results() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;
    insert_test_task(
        &mut conn,
        "7KQ9A1X4MV2P8D6R",
        "Needle in title",
        "todo",
        "none",
        "001",
    )
    .await;
    insert_test_task(
        &mut conn,
        "9KQ9A1X4MV2P8D6R",
        "Deleted needle",
        "todo",
        "none",
        "002",
    )
    .await;
    sqlx::query("UPDATE tasks SET deleted = 1 WHERE id = '9KQ9A1X4MV2P8D6R'")
        .execute(&mut *conn)
        .await
        .unwrap();

    let by_ref = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "7KQ9".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(by_ref[0].item.task.id.as_str(), "7KQ9A1X4MV2P8D6R");
    assert_eq!(by_ref[0].matched_field, SearchMatchedField::Ref);

    let qualified_ref = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "/APP-7KQ9".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(qualified_ref.len(), 1);
    assert_eq!(qualified_ref[0].item.task.id.as_str(), "7KQ9A1X4MV2P8D6R");
    assert_eq!(qualified_ref[0].matched_field, SearchMatchedField::Ref);

    let without_deleted = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "deleted needle".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert!(without_deleted.is_empty());

    let with_deleted = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "deleted needle".to_string(),
            include_deleted: true,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(listed_titles_from_search(&with_deleted), ["Deleted needle"]);
    assert!(with_deleted[0].item.task.deleted);

    let single_token_without_deleted = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "needle".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        listed_titles_from_search(&single_token_without_deleted),
        ["Needle in title"]
    );
    assert!(
        single_token_without_deleted
            .iter()
            .all(|result| !result.item.task.deleted)
    );

    let deleted_by_ref = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "9KQ9".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(deleted_by_ref.len(), 1);
    assert_eq!(deleted_by_ref[0].item.task.id.as_str(), "9KQ9A1X4MV2P8D6R");
    assert_eq!(deleted_by_ref[0].matched_field, SearchMatchedField::Ref);
    assert!(deleted_by_ref[0].item.task.deleted);
}

#[tokio::test]
async fn task_search_accepts_unsafe_query_parser_input() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;
    insert_test_task(
        &mut conn,
        "7KQ9A1X4MV2P8D6R",
        "Pager rotation cleanup",
        "todo",
        "none",
        "001",
    )
    .await;

    for input in [
        "",
        "\"",
        "\"\"",
        "(",
        ")",
        "a*b",
        "\"(",
        "AND OR NOT",
        ":",
        "/",
        "-",
        "a:b",
        "\"unfinished",
        "x OR y",
    ] {
        search_task_items_in_workspace(
            &mut conn,
            &crate::workspaces::default_workspace_id(),
            TaskSearchQuery {
                text: input.to_string(),
                include_deleted: false,
                limit: 5,
            },
        )
        .await
        .expect("search input must parse and search safely");
    }
}

#[tokio::test]
async fn task_search_finds_rows_backfilled_into_fts_index() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;
    insert_test_task(
        &mut conn,
        "7KQ9A1X4MV2P8D6R",
        "Backfilled migration text",
        "todo",
        "none",
        "001",
    )
    .await;

    let items = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "backfilled".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();

    assert_eq!(
        listed_titles_from_search(&items),
        ["Backfilled migration text"]
    );
}

#[tokio::test]
async fn task_search_uses_fts_candidates_for_common_text() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;
    insert_test_task(
        &mut conn,
        "7KQ9A1X4MV2P8D6R",
        "Needle visible in fts",
        "todo",
        "none",
        "001",
    )
    .await;

    // Use a multi-word query so it is NOT treated as a ref query by the parser.
    // Multi-word queries route through the FTS path.
    let before = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "visible in".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        listed_titles_from_search(&before),
        ["Needle visible in fts"]
    );

    // Delete the FTS entry directly. This removes the FTS index entry
    // while keeping the content table and tasks row intact.
    sqlx::query(
        "DELETE FROM task_search_fts WHERE rowid = (SELECT doc_id FROM task_search_documents WHERE task_id = '7KQ9A1X4MV2P8D6R')",
    )
    .execute(&mut *conn)
    .await
    .unwrap();

    // After FTS entry is deleted, search should return nothing because
    // the FTS path is used for multi-word text queries.
    let after = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "visible in".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert!(after.is_empty());
}

#[tokio::test]
async fn task_search_fts_updates_when_search_text_changes() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;
    insert_test_task(
        &mut conn,
        "7KQ9A1X4MV2P8D6R",
        "Original title",
        "todo",
        "none",
        "001",
    )
    .await;

    sqlx::query(
        "UPDATE tasks SET title = 'Retitled orchard', description = 'body lagoon' WHERE id = '7KQ9A1X4MV2P8D6R'",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        listed_titles_from_search(
            &search_task_items_in_workspace(
                &mut conn,
                &crate::workspaces::default_workspace_id(),
                TaskSearchQuery {
                    text: "orchard".to_string(),
                    include_deleted: false,
                    limit: 10,
                },
            )
            .await
            .unwrap()
        ),
        ["Retitled orchard"]
    );
    assert_eq!(
        listed_titles_from_search(
            &search_task_items_in_workspace(
                &mut conn,
                &crate::workspaces::default_workspace_id(),
                TaskSearchQuery {
                    text: "lagoon".to_string(),
                    include_deleted: false,
                    limit: 10,
                },
            )
            .await
            .unwrap()
        ),
        ["Retitled orchard"]
    );

    insert_test_label(&mut conn, "7KQ9A1X4MV2P8D6R", "garden").await;
    assert_eq!(
        listed_titles_from_search(
            &search_task_items_in_workspace(
                &mut conn,
                &crate::workspaces::default_workspace_id(),
                TaskSearchQuery {
                    text: "garden".to_string(),
                    include_deleted: false,
                    limit: 10,
                },
            )
            .await
            .unwrap()
        ),
        ["Retitled orchard"]
    );
    sqlx::query("DELETE FROM task_labels WHERE task_id = '7KQ9A1X4MV2P8D6R' AND label = 'garden'")
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(
        search_task_items_in_workspace(
            &mut conn,
            &crate::workspaces::default_workspace_id(),
            TaskSearchQuery {
                text: "garden".to_string(),
                include_deleted: false,
                limit: 10,
            },
        )
        .await
        .unwrap()
        .is_empty()
    );

    sqlx::query(
        "INSERT INTO notes(id, task_id, body, created_at, change_id) VALUES ('note-fts', '7KQ9A1X4MV2P8D6R', 'harbor memo', '002', 'change-fts')",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        listed_titles_from_search(
            &search_task_items_in_workspace(
                &mut conn,
                &crate::workspaces::default_workspace_id(),
                TaskSearchQuery {
                    text: "harbor".to_string(),
                    include_deleted: false,
                    limit: 10,
                },
            )
            .await
            .unwrap()
        ),
        ["Retitled orchard"]
    );
    sqlx::query("UPDATE notes SET body = 'summit memo' WHERE id = 'note-fts'")
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(
        search_task_items_in_workspace(
            &mut conn,
            &crate::workspaces::default_workspace_id(),
            TaskSearchQuery {
                text: "harbor".to_string(),
                include_deleted: false,
                limit: 10,
            },
        )
        .await
        .unwrap()
        .is_empty()
    );
    assert_eq!(
        listed_titles_from_search(
            &search_task_items_in_workspace(
                &mut conn,
                &crate::workspaces::default_workspace_id(),
                TaskSearchQuery {
                    text: "summit".to_string(),
                    include_deleted: false,
                    limit: 10,
                },
            )
            .await
            .unwrap()
        ),
        ["Retitled orchard"]
    );

    sqlx::query("UPDATE projects SET name = 'Beacon Project' WHERE key = 'app'")
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        listed_titles_from_search(
            &search_task_items_in_workspace(
                &mut conn,
                &crate::workspaces::default_workspace_id(),
                TaskSearchQuery {
                    text: "beacon".to_string(),
                    include_deleted: false,
                    limit: 10,
                },
            )
            .await
            .unwrap()
        ),
        ["Retitled orchard"]
    );
}

#[tokio::test]
async fn task_search_ref_lane_keeps_glyph_normalization_out_of_text_lanes() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;
    insert_test_task(
        &mut conn,
        "7KQ9A1X4MV2P8D6R",
        "looking glass",
        "todo",
        "none",
        "001",
    )
    .await;

    let text = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "100king".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert!(text.is_empty());
}

#[tokio::test]
async fn task_search_ref_lane_handles_durable_ids_and_punctuation() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;
    insert_test_task(
        &mut conn,
        "7KQ9A1X4MV2P8D6R",
        "Needle in title",
        "todo",
        "none",
        "001",
    )
    .await;
    insert_test_task(
        &mut conn,
        "9KQ9A1X4MV2P8D6R",
        "Deleted needle",
        "todo",
        "none",
        "002",
    )
    .await;
    sqlx::query("UPDATE tasks SET deleted = 1 WHERE id = '9KQ9A1X4MV2P8D6R'")
        .execute(&mut *conn)
        .await
        .unwrap();

    let durable = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "7KQ9A1X4MV2P8D6R".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(durable[0].item.task.id.as_str(), "7KQ9A1X4MV2P8D6R");
    assert_eq!(durable[0].matched_field, SearchMatchedField::Ref);

    let punctuated = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "/APP.7KQ9".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(punctuated[0].item.task.id.as_str(), "7KQ9A1X4MV2P8D6R");
    assert_eq!(punctuated[0].matched_field, SearchMatchedField::Ref);

    let spaced = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "APP 7KQ9".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(spaced[0].item.task.id.as_str(), "7KQ9A1X4MV2P8D6R");
    assert_eq!(spaced[0].matched_field, SearchMatchedField::Ref);

    let wrong_prefix = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "/WRONG-7KQ9".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert!(wrong_prefix.is_empty());

    let short_deleted = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "9KQ".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert!(short_deleted.is_empty());

    let query_also_in_title = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "needle".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        query_also_in_title[0].matched_field,
        SearchMatchedField::Title
    );
}

#[tokio::test]
async fn task_search_reranks_title_ref_label_note_and_metadata_evidence() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;
    insert_test_task(
        &mut conn,
        "7KQ9A1X4MV2P8D6R",
        "Orchard release",
        "todo",
        "medium",
        "001",
    )
    .await;
    insert_test_task(
        &mut conn,
        "8KQ9A1X4MV2P8D6R",
        "Label and note",
        "todo",
        "none",
        "002",
    )
    .await;
    insert_test_task(
        &mut conn,
        "9KQ9A1X4MV2P8D6R",
        "Project metadata",
        "todo",
        "urgent",
        "003",
    )
    .await;
    insert_test_task(
        &mut conn,
        "AKQ9A1X4MV2P8D6R",
        "Note only",
        "todo",
        "none",
        "004",
    )
    .await;
    insert_test_label(&mut conn, "8KQ9A1X4MV2P8D6R", "orchard").await;
    sqlx::query(
        "INSERT INTO notes(id, task_id, body, created_at, change_id)
         VALUES ('note-rerank-a', '8KQ9A1X4MV2P8D6R', 'orchard detail', '002', 'change-rerank-a'),
                ('note-rerank-b', 'AKQ9A1X4MV2P8D6R', 'orchard detail', '004', 'change-rerank-b')",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO projects(id, key, name, prefix, created_at, updated_at)
         VALUES ('0000000000000002', 'meta', 'Orchard Metadata', 'MET', 't', 't')",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("UPDATE tasks SET project_id = '0000000000000002' WHERE id = '9KQ9A1X4MV2P8D6R'")
        .execute(&mut *conn)
        .await
        .unwrap();

    let items = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "orchard".to_string(),
            include_deleted: true,
            limit: 10,
        },
    )
    .await
    .unwrap();

    let position = |title: &str| {
        items
            .iter()
            .position(|item| item.item.task.title == title)
            .unwrap()
    };
    let title = position("Orchard release");
    let label = position("Label and note");
    let project = position("Project metadata");
    let note = position("Note only");

    assert!(title < label);
    assert!(label < project);
    assert!(project < note);
    assert_eq!(items[title].matched_field, SearchMatchedField::Title);
    assert_eq!(items[label].matched_field, SearchMatchedField::Label);
    assert_eq!(items[project].matched_field, SearchMatchedField::Project);
    assert_eq!(items[note].matched_field, SearchMatchedField::Note);

    let by_ref = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "7KQ9".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(by_ref[0].item.task.id.as_str(), "7KQ9A1X4MV2P8D6R");
    assert_eq!(by_ref[0].matched_field, SearchMatchedField::Ref);
}

#[tokio::test]
async fn task_search_reranks_multi_token_proximity_and_field_count() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;
    insert_test_task(
        &mut conn,
        "7KQ9A1X4MV2P8D6R",
        "Auth flow",
        "todo",
        "none",
        "001",
    )
    .await;
    insert_test_task(
        &mut conn,
        "8KQ9A1X4MV2P8D6R",
        "Metadata bridge",
        "todo",
        "none",
        "002",
    )
    .await;
    insert_test_task(
        &mut conn,
        "9KQ9A1X4MV2P8D6R",
        "Note spread",
        "todo",
        "urgent",
        "003",
    )
    .await;
    insert_test_label(&mut conn, "8KQ9A1X4MV2P8D6R", "auth").await;
    insert_test_label(&mut conn, "8KQ9A1X4MV2P8D6R", "flow").await;
    sqlx::query(
        "INSERT INTO notes(id, task_id, body, created_at, change_id)
         VALUES ('note-token-a', '9KQ9A1X4MV2P8D6R', 'auth text with several unrelated words before flow', '003', 'change-token-a')",
    )
    .execute(&mut *conn)
    .await
    .unwrap();

    let items = search_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskSearchQuery {
            text: "auth flow".to_string(),
            include_deleted: false,
            limit: 10,
        },
    )
    .await
    .unwrap();

    assert_eq!(
        listed_titles_from_search(&items),
        ["Auth flow", "Metadata bridge", "Note spread"]
    );
    assert_eq!(items[0].matched_field, SearchMatchedField::Title);
    assert_eq!(items[1].matched_field, SearchMatchedField::Label);
    assert_eq!(items[2].matched_field, SearchMatchedField::Note);
}
