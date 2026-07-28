use crate::ids::{TaskId, WorkspaceId};
use anyhow::Result;
use chrono::Local;
use serde::Serialize;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};
use std::collections::HashMap;

use crate::db::task_from_row;
use crate::refs::DisplayRefContext;
use crate::task_enrichment::{epic_parents_for_tasks, labels_for_tasks};
use crate::types::Task;

use super::TaskListItem;
use super::hydration::build_task_list_items;

mod parser;

const SQLITE_BIND_CHUNK_SIZE: usize = 900;

const DEFAULT_LIMIT: usize = 50;
const REF_WEIGHT: i64 = 1_000;
const TITLE_WEIGHT: i64 = 420;
const LABEL_WEIGHT: i64 = 240;
const PROJECT_WEIGHT: i64 = 220;
const STATUS_WEIGHT: i64 = 160;
const PRIORITY_WEIGHT: i64 = 150;
const DESCRIPTION_WEIGHT: i64 = 100;
const ATTACHMENT_WEIGHT: i64 = 90;
const NOTE_WEIGHT: i64 = 80;
const FIELD_MATCH_BONUS: i64 = 35_000;
const EXTRA_FIELD_BONUS: i64 = 18_000;
const FIELD_SCORE_DIVISOR: i64 = 5;
const PRIORITY_BOOST_CAP: i64 = 18_000;
const RECENCY_BOOST_CAP: i64 = 12_000;

#[derive(Debug, Clone)]
pub struct TaskSearchQuery {
    pub text: String,
    pub include_deleted: bool,
    pub limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMatchedField {
    Ref,
    Title,
    Label,
    Project,
    Status,
    Priority,
    Description,
    Attachment,
    Note,
}

impl SearchMatchedField {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ref => "ref",
            Self::Title => "title",
            Self::Label => "label",
            Self::Project => "project",
            Self::Status => "status",
            Self::Priority => "priority",
            Self::Description => "description",
            Self::Attachment => "attachment",
            Self::Note => "note",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskSearchResult {
    pub item: TaskListItem,
    pub score: i64,
    pub matched_field: SearchMatchedField,
    pub snippet: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TaskSearchResultSet {
    pub items: Vec<TaskSearchResult>,
}

#[derive(Debug, Clone)]
pub struct TaskSearchPreviewResult {
    pub task_id: TaskId,
    pub display_ref: String,
    pub title: String,
    pub project_key: String,
    pub status: String,
    pub priority: String,
    pub created_at: String,
    pub labels: Vec<String>,
    pub deleted: bool,
    pub is_epic: bool,
    pub epic_parent_display_ref: Option<String>,
    pub score: i64,
    pub matched_field: SearchMatchedField,
    pub snippet: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TaskSearchPreviewResultSet {
    pub items: Vec<TaskSearchPreviewResult>,
    pub total_matches: usize,
}

struct ScoredSearchResults {
    items: Vec<ScoredDocument>,
    total_matches: usize,
}

struct SearchDocument {
    task: Task,
    display_ref: String,
    project_name: String,
    labels_text: String,
    notes_text: String,
    attachments_text: String,
}

struct ScoredDocument {
    document: SearchDocument,
    score: i64,
    matched_field: SearchMatchedField,
    snippet: Option<String>,
}

struct FieldEvidence {
    score: i64,
    matched_field: SearchMatchedField,
    snippet: Option<String>,
}

pub async fn search_task_items_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    query: TaskSearchQuery,
) -> Result<Vec<TaskSearchResult>> {
    Ok(search_task_item_set_in_workspace(conn, workspace_id, query)
        .await?
        .items)
}

pub async fn search_task_item_set_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    query: TaskSearchQuery,
) -> Result<TaskSearchResultSet> {
    let display_refs = DisplayRefContext::for_workspace(conn, workspace_id).await?;
    let scored = scored_search_documents(conn, workspace_id, &query, &display_refs).await?;
    let tasks = scored
        .items
        .iter()
        .map(|scored| scored.document.task.clone())
        .collect::<Vec<_>>();
    let now_seconds = crate::queue::now_seconds();
    let items = build_task_list_items(
        conn,
        workspace_id,
        tasks,
        now_seconds,
        Local::now().date_naive(),
        &display_refs,
    )
    .await?;
    let by_id = items
        .into_iter()
        .map(|item| (item.task.id.clone(), item))
        .collect::<HashMap<_, _>>();
    let results = scored
        .items
        .into_iter()
        .filter_map(|scored| {
            by_id
                .get(&scored.document.task.id)
                .cloned()
                .map(|item| TaskSearchResult {
                    item,
                    score: scored.score,
                    matched_field: scored.matched_field,
                    snippet: scored.snippet,
                })
        })
        .collect();
    Ok(TaskSearchResultSet { items: results })
}

async fn scored_search_documents(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    query: &TaskSearchQuery,
    display_refs: &DisplayRefContext,
) -> Result<ScoredSearchResults> {
    let limit = if query.limit == 0 {
        DEFAULT_LIMIT
    } else {
        query.limit
    };
    let parsed = parser::parse_task_search_query(&query.text);
    if parsed.trimmed.is_empty() {
        return Ok(ScoredSearchResults {
            items: Vec::new(),
            total_matches: 0,
        });
    }
    let load_deleted = query.include_deleted || parsed.ref_query.is_some();
    let documents =
        load_candidate_search_documents(conn, workspace_id, load_deleted, &parsed, display_refs)
            .await?;

    let now_seconds = crate::queue::now_seconds();
    let mut scored = documents
        .into_iter()
        .filter_map(|document| {
            let is_deleted = document.task.deleted;
            let ref_strong_enough = parsed
                .ref_query
                .as_ref()
                .is_some_and(|rq| ref_query_matches_display_or_full_id(&document, rq));
            let scored = score_document(document, &parsed, now_seconds)?;
            if is_deleted
                && !query.include_deleted
                && (scored.matched_field != SearchMatchedField::Ref || !ref_strong_enough)
            {
                return None;
            }
            Some(scored)
        })
        .collect::<Vec<_>>();
    let total_matches = scored.len();
    scored.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.document.task.updated_at.cmp(&a.document.task.updated_at))
            .then_with(|| a.document.task.title.cmp(&b.document.task.title))
            .then_with(|| a.document.task.id.cmp(&b.document.task.id))
    });
    scored.truncate(limit);
    Ok(ScoredSearchResults {
        items: scored,
        total_matches,
    })
}

pub async fn search_task_preview_set_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    query: TaskSearchQuery,
) -> Result<TaskSearchPreviewResultSet> {
    let display_refs = DisplayRefContext::for_workspace(conn, workspace_id).await?;
    let scored = scored_search_documents(conn, workspace_id, &query, &display_refs).await?;
    let task_ids = scored
        .items
        .iter()
        .map(|scored| scored.document.task.id.clone())
        .collect::<Vec<_>>();
    let mut labels_by_task = labels_for_tasks(conn, workspace_id, &task_ids).await?;
    let mut epic_parents_by_task =
        epic_parents_for_tasks(conn, workspace_id, &task_ids, &display_refs).await?;
    let items = scored
        .items
        .into_iter()
        .map(|scored| {
            let task = scored.document.task;
            TaskSearchPreviewResult {
                task_id: task.id.clone(),
                display_ref: scored.document.display_ref,
                title: task.title,
                project_key: task.project_key,
                status: task.status.as_str().to_string(),
                priority: task.priority.as_str().to_string(),
                created_at: task.created_at,
                labels: labels_by_task.remove(&task.id).unwrap_or_default(),
                deleted: task.deleted,
                is_epic: task.is_epic,
                epic_parent_display_ref: epic_parents_by_task
                    .remove(&task.id)
                    .map(|parent| parent.display_ref),
                score: scored.score,
                matched_field: scored.matched_field,
                snippet: scored.snippet,
            }
        })
        .collect();
    Ok(TaskSearchPreviewResultSet {
        items,
        total_matches: scored.total_matches,
    })
}

async fn attachment_search_text_by_task(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_ids: &[String],
) -> Result<HashMap<String, String>> {
    let mut by_task: HashMap<String, Vec<String>> = HashMap::new();
    if task_ids.is_empty() {
        return Ok(HashMap::new());
    }
    for chunk in task_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        if chunk.is_empty() {
            continue;
        }
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT task_id, filename, alt_text FROM task_attachments WHERE workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND deleted = 0 AND task_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(") ORDER BY task_id, created_at, attachment_id");

        for row in query.build().fetch_all(&mut *conn).await? {
            let task_id: String = row.get("task_id");
            if let Some(filename) = row.get::<Option<String>, _>("filename") {
                by_task.entry(task_id.clone()).or_default().push(filename);
            }
            if let Some(alt_text) = row.get::<Option<String>, _>("alt_text") {
                by_task.entry(task_id).or_default().push(alt_text);
            }
        }
    }
    Ok(by_task
        .into_iter()
        .map(|(task_id, parts)| (task_id, parts.join(" ")))
        .collect())
}

async fn attach_attachment_search_text(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    documents: &mut [SearchDocument],
) -> Result<()> {
    let task_ids = documents
        .iter()
        .map(|document| document.task.id.to_string())
        .collect::<Vec<_>>();
    let mut attachments_by_task =
        attachment_search_text_by_task(conn, workspace_id, &task_ids).await?;
    for document in documents {
        document.attachments_text = attachments_by_task
            .remove(document.task.id.as_str())
            .unwrap_or_default();
    }
    Ok(())
}

fn attachment_query_terms(parsed: &parser::ParsedTaskSearchQuery) -> Vec<String> {
    let mut terms = Vec::new();
    let trimmed = parsed.trimmed.trim().to_ascii_lowercase();
    if !trimmed.is_empty() {
        terms.push(trimmed);
    }
    for term in search_terms(parsed) {
        let term = term.trim().to_ascii_lowercase();
        if !term.is_empty() && !terms.iter().any(|existing| existing == &term) {
            terms.push(term);
        }
    }
    terms
}

async fn load_attachment_text_search_documents(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    include_deleted: bool,
    parsed: &parser::ParsedTaskSearchQuery,
    display_refs: &DisplayRefContext,
) -> Result<Vec<SearchDocument>> {
    let terms = attachment_query_terms(parsed);
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT DISTINCT t.id, t.workspace_id, t.title, t.description, t.project_id,
         p.key AS project_key, p.name AS project_name, p.prefix AS project_prefix,
         t.status, t.priority, t.source, t.created_at, t.updated_at, t.queue_activity_at, t.available_at, t.due_on, t.deleted, t.is_epic,
         '' AS fts_labels, '' AS fts_notes
         FROM task_attachments ta
         JOIN tasks t ON t.workspace_id = ta.workspace_id AND t.id = ta.task_id
         JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
         WHERE ta.workspace_id = ",
    );
    query.push_bind(workspace_id);
    query.push(" AND ta.deleted = 0 AND (");
    let mut first = true;
    for term in terms {
        if !first {
            query.push(" OR ");
        }
        first = false;
        let pattern = format!("%{term}%");
        query.push("(lower(COALESCE(ta.filename, '')) LIKE ");
        query.push_bind(pattern.clone());
        query.push(" OR lower(COALESCE(ta.alt_text, '')) LIKE ");
        query.push_bind(pattern);
        query.push(")");
    }
    query.push(") AND (");
    query.push_bind(include_deleted);
    query.push(" OR t.deleted = 0) ORDER BY t.updated_at DESC, t.id");

    let rows = query.build().fetch_all(&mut *conn).await?;
    search_documents_from_rows(rows, display_refs)
}

async fn load_candidate_search_documents(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    include_deleted: bool,
    parsed: &parser::ParsedTaskSearchQuery,
    display_refs: &DisplayRefContext,
) -> Result<Vec<SearchDocument>> {
    let mut documents = if let Some(fts_match) = parsed.fts_match.as_deref() {
        load_fts_search_documents(conn, workspace_id, include_deleted, fts_match, display_refs)
            .await?
    } else {
        Vec::new()
    };
    if let Some(ref_query) = &parsed.ref_query {
        let ref_documents =
            load_ref_search_documents(conn, workspace_id, include_deleted, ref_query, display_refs)
                .await?;
        merge_search_documents(&mut documents, ref_documents);
    }
    let attachment_documents = load_attachment_text_search_documents(
        conn,
        workspace_id.as_str(),
        include_deleted,
        parsed,
        display_refs,
    )
    .await?;
    merge_search_documents(&mut documents, attachment_documents);
    attach_attachment_search_text(conn, workspace_id.as_str(), &mut documents).await?;
    Ok(documents)
}

async fn load_ref_search_documents(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    include_deleted: bool,
    ref_query: &parser::ParsedRefSearchQuery,
    display_refs: &DisplayRefContext,
) -> Result<Vec<SearchDocument>> {
    let rows = sqlx::query(
        "SELECT t.id, t.workspace_id, t.title, t.description, t.project_id,
         p.key AS project_key, p.name AS project_name, p.prefix AS project_prefix,
         t.status, t.priority, t.source, t.created_at, t.updated_at, t.queue_activity_at, t.available_at, t.due_on, t.deleted, t.is_epic,
         '' AS fts_labels, '' AS fts_notes
         FROM tasks t JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
         WHERE t.workspace_id = ? AND (? OR t.deleted = 0) AND t.id LIKE ? || '%'
         ORDER BY t.updated_at DESC, t.id",
    )
    .bind(workspace_id)
    .bind(include_deleted)
    .bind(&ref_query.normalized_suffix)
    .fetch_all(&mut *conn)
    .await?;
    search_documents_from_rows(rows, display_refs)
}

fn merge_search_documents(documents: &mut Vec<SearchDocument>, incoming: Vec<SearchDocument>) {
    for document in incoming {
        if !documents
            .iter()
            .any(|existing| existing.task.id == document.task.id)
        {
            documents.push(document);
        }
    }
}

async fn load_fts_search_documents(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    include_deleted: bool,
    raw_fts_match: &str,
    display_refs: &DisplayRefContext,
) -> Result<Vec<SearchDocument>> {
    let fts_match = workspace_scoped_fts_match(workspace_id, raw_fts_match);
    let rows = sqlx::query(
        "SELECT t.id, t.workspace_id, t.title, t.description, t.project_id,
         p.key AS project_key, p.name AS project_name, p.prefix AS project_prefix,
         t.status, t.priority, t.source, t.created_at, t.updated_at, t.queue_activity_at, t.available_at, t.due_on, t.deleted, t.is_epic,
         d.labels AS fts_labels, d.notes AS fts_notes
         FROM task_search_fts f
         JOIN task_search_documents d ON d.doc_id = f.rowid
         JOIN tasks t ON t.workspace_id = d.workspace_id AND t.id = d.task_id
         JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
         WHERE task_search_fts MATCH ? AND d.workspace_id = ? AND (? OR t.deleted = 0)
         ORDER BY t.updated_at DESC, t.id",
    )
    .bind(&fts_match)
    .bind(workspace_id)
    .bind(include_deleted)
    .fetch_all(&mut *conn)
    .await?;
    search_documents_from_rows(rows, display_refs)
}

fn search_documents_from_rows(
    rows: Vec<sqlx::sqlite::SqliteRow>,
    display_refs: &DisplayRefContext,
) -> Result<Vec<SearchDocument>> {
    let mut tasks = Vec::with_capacity(rows.len());
    let mut project_names = Vec::with_capacity(rows.len());
    let mut labels_texts = Vec::with_capacity(rows.len());
    let mut notes_texts = Vec::with_capacity(rows.len());
    for row in rows {
        project_names.push(row.get::<String, _>("project_name"));
        labels_texts.push(row.get::<String, _>("fts_labels"));
        notes_texts.push(row.get::<String, _>("fts_notes"));
        tasks.push(task_from_row(&row)?);
    }
    Ok(tasks
        .into_iter()
        .zip(project_names)
        .zip(labels_texts)
        .zip(notes_texts)
        .map(|(((task, project_name), labels_text), notes_text)| {
            let display_ref = display_refs.display_ref(&task);
            SearchDocument {
                labels_text,
                notes_text,
                attachments_text: String::new(),
                task,
                display_ref,
                project_name,
            }
        })
        .collect())
}

fn fts_phrase(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn workspace_scoped_fts_match(workspace_id: &WorkspaceId, fts_match: &str) -> String {
    format!(
        "workspace_token:{} {}",
        fts_phrase(workspace_id.as_str()),
        fts_match
    )
}

fn score_document(
    document: SearchDocument,
    query: &parser::ParsedTaskSearchQuery,
    now_seconds: i64,
) -> Option<ScoredDocument> {
    let project_text = format!(
        "{} {} {}",
        document.task.project_key, document.project_name, document.task.project_prefix
    );
    let mut evidence = Vec::new();
    if let Some(ref_query) = &query.ref_query
        && let Some(score) = score_ref_lane(&document, ref_query)
    {
        evidence.push(FieldEvidence {
            score,
            matched_field: SearchMatchedField::Ref,
            snippet: None,
        });
    }
    for (field, text, weight) in [
        (
            SearchMatchedField::Title,
            document.task.title.as_str(),
            TITLE_WEIGHT,
        ),
        (
            SearchMatchedField::Label,
            document.labels_text.as_str(),
            LABEL_WEIGHT,
        ),
        (
            SearchMatchedField::Project,
            project_text.as_str(),
            PROJECT_WEIGHT,
        ),
        (
            SearchMatchedField::Status,
            document.task.status.as_str(),
            STATUS_WEIGHT,
        ),
        (
            SearchMatchedField::Priority,
            document.task.priority.as_str(),
            PRIORITY_WEIGHT,
        ),
        (
            SearchMatchedField::Description,
            document.task.description.as_str(),
            DESCRIPTION_WEIGHT,
        ),
        (
            SearchMatchedField::Attachment,
            document.attachments_text.as_str(),
            ATTACHMENT_WEIGHT,
        ),
        (
            SearchMatchedField::Note,
            document.notes_text.as_str(),
            NOTE_WEIGHT,
        ),
    ] {
        if let Some((score, span)) = score_text_lane(text, query) {
            let snippet = if matches!(
                field,
                SearchMatchedField::Description | SearchMatchedField::Note
            ) {
                snippet(text, span)
            } else {
                None
            };
            evidence.push(FieldEvidence {
                score: score * weight,
                matched_field: field,
                snippet,
            });
        }
    }
    if evidence.is_empty() {
        return None;
    }
    let best_index = evidence
        .iter()
        .enumerate()
        .max_by(|(_, left), (_, right)| left.score.cmp(&right.score))
        .map(|(index, _)| index)
        .unwrap();
    let best = evidence.swap_remove(best_index);
    let extra_score = evidence
        .iter()
        .map(|item| item.score / FIELD_SCORE_DIVISOR)
        .sum::<i64>();
    let field_bonus = FIELD_MATCH_BONUS + evidence.len() as i64 * EXTRA_FIELD_BONUS;
    let score = best.score
        + extra_score
        + field_bonus
        + priority_boost(document.task.priority.as_str())
        + recency_boost(document.task.updated_at.as_str(), now_seconds);
    Some(ScoredDocument {
        document,
        score,
        matched_field: best.matched_field,
        snippet: best.snippet,
    })
}

fn priority_boost(priority: &str) -> i64 {
    match priority {
        "urgent" => PRIORITY_BOOST_CAP,
        "high" => 12_000,
        "medium" => 6_000,
        "low" => 2_000,
        _ => 0,
    }
}

fn recency_boost(updated_at: &str, now_seconds: i64) -> i64 {
    let Some(updated_seconds) = crate::queue::unix_seconds(updated_at) else {
        return 0;
    };
    let age_days = now_seconds.saturating_sub(updated_seconds).max(0) / 86_400;
    let decay = age_days.saturating_mul(RECENCY_BOOST_CAP / 30);
    RECENCY_BOOST_CAP.saturating_sub(decay)
}

fn score_text_lane(
    text: &str,
    query: &parser::ParsedTaskSearchQuery,
) -> Option<(i64, std::ops::Range<usize>)> {
    if query.phrases.is_empty() {
        score_contiguous_text_lane(text, query.trimmed.as_str())
            .or_else(|| score_term_coverage_lane(text, query))
    } else {
        score_parsed_contiguous_text_lane(text, query)
            .or_else(|| score_term_coverage_lane(text, query))
    }
}

fn score_parsed_contiguous_text_lane(
    text: &str,
    query: &parser::ParsedTaskSearchQuery,
) -> Option<(i64, std::ops::Range<usize>)> {
    search_terms(query)
        .into_iter()
        .filter_map(|term| score_contiguous_text_lane(text, term))
        .max_by_key(|(score, _)| *score)
}

fn score_contiguous_text_lane(text: &str, query: &str) -> Option<(i64, std::ops::Range<usize>)> {
    let normalized_text = text.to_ascii_lowercase();
    let raw_query = query.trim();
    let normalized_query = raw_query.to_ascii_lowercase();
    let query = normalized_query.trim();
    if query.is_empty() || normalized_text.is_empty() {
        return None;
    }
    if let Some(index) = normalized_text.find(query) {
        let boundary_bonus = if index == 0 || is_boundary(normalized_text.as_bytes()[index - 1]) {
            200
        } else {
            0
        };
        let phrase_bonus = if index == 0 { 300 } else { 0 };
        return Some((
            1_000 + phrase_bonus + boundary_bonus - index as i64,
            index..index + query.len(),
        ));
    }
    token_match_span(&normalized_text, query).map(|span| {
        let boundary_bonus =
            if span.start == 0 || is_boundary(normalized_text.as_bytes()[span.start - 1]) {
                120
            } else {
                0
            };
        let spread = span.end.saturating_sub(span.start + query.len()) as i64;
        (700 + boundary_bonus - spread * 4 - span.start as i64, span)
    })
}

fn score_term_coverage_lane(
    text: &str,
    query: &parser::ParsedTaskSearchQuery,
) -> Option<(i64, std::ops::Range<usize>)> {
    let terms = search_terms(query);
    let normalized_text = text.to_ascii_lowercase();
    if terms.len() < 2 || normalized_text.is_empty() {
        return None;
    }

    let mut matched = 0_i64;
    let mut start = usize::MAX;
    let mut end = 0_usize;
    for term in terms {
        let normalized_term = term.to_ascii_lowercase();
        if normalized_term.is_empty() {
            continue;
        }
        if let Some(index) = normalized_text.find(&normalized_term) {
            matched += 1;
            start = start.min(index);
            end = end.max(index + normalized_term.len());
        }
    }
    if matched == 0 {
        return None;
    }

    let boundary_bonus = if start == 0 || is_boundary(normalized_text.as_bytes()[start - 1]) {
        120
    } else {
        0
    };
    let spread = end.saturating_sub(start) as i64;
    Some((
        450 + matched * 160 + boundary_bonus - spread * 3 - start as i64,
        start..end,
    ))
}

fn search_terms(query: &parser::ParsedTaskSearchQuery) -> Vec<&str> {
    query
        .phrases
        .iter()
        .map(String::as_str)
        .chain(query.tokens.iter().map(String::as_str))
        .chain(query.active_prefix.as_deref())
        .collect()
}

fn score_ref_lane(
    document: &SearchDocument,
    ref_query: &parser::ParsedRefSearchQuery,
) -> Option<i64> {
    if let Some(prefix) = ref_query.normalized_prefix.as_deref()
        && normalize_ref_query(&document.task.project_prefix) != prefix
    {
        return None;
    }
    let normalized_id = normalize_ref_query(&document.task.id);
    if !normalized_id.starts_with(&ref_query.normalized_suffix) {
        return None;
    }
    let display_suffix_len = document
        .display_ref
        .rsplit_once('-')
        .map(|(_, suffix)| normalize_ref_query(suffix).len())
        .unwrap_or(0);
    let exact_bonus = if normalized_id == ref_query.normalized_suffix {
        700
    } else {
        0
    };
    let display_bonus = if ref_query.normalized_suffix.len() >= display_suffix_len {
        300
    } else {
        0
    };
    let prefix_bonus = if ref_query.normalized_prefix.is_some() {
        200
    } else {
        0
    };
    Some(
        (3_000
            + exact_bonus
            + display_bonus
            + prefix_bonus
            + ref_query.normalized_suffix.len() as i64)
            * REF_WEIGHT,
    )
}

fn ref_query_matches_display_or_full_id(
    document: &SearchDocument,
    ref_query: &parser::ParsedRefSearchQuery,
) -> bool {
    let normalized_id = normalize_ref_query(&document.task.id);
    if normalized_id == ref_query.normalized_suffix {
        return true;
    }
    document
        .display_ref
        .rsplit_once('-')
        .map(|(_, suffix)| ref_query.normalized_suffix.len() >= normalize_ref_query(suffix).len())
        .unwrap_or(false)
}

fn token_match_span(text: &str, query: &str) -> Option<std::ops::Range<usize>> {
    let tokens = query.split_whitespace().collect::<Vec<_>>();
    if tokens.len() < 2 {
        return None;
    }
    let mut start = usize::MAX;
    let mut end = 0;
    for token in tokens {
        let index = text.find(token)?;
        start = start.min(index);
        end = end.max(index + token.len());
    }
    Some(start..end)
}

fn normalize_ref_query(input: &str) -> String {
    input
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| match ch.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            ch => ch,
        })
        .collect()
}

fn is_boundary(ch: u8) -> bool {
    !ch.is_ascii_alphanumeric()
}

fn snippet(text: &str, span: std::ops::Range<usize>) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    let start = char_boundary_at_or_before(text, span.start.saturating_sub(40));
    let end = char_boundary_at_or_after(text, (span.end + 80).min(text.len()));
    let mut value = text[start..end].replace('\n', " ");
    value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if start > 0 {
        value.insert_str(0, "...");
    }
    if end < text.len() {
        value.push_str("...");
    }
    Some(value)
}

fn char_boundary_at_or_before(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn char_boundary_at_or_after(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
