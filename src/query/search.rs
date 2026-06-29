use anyhow::Result;
use serde::Serialize;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};
use std::collections::HashMap;

use crate::db::task_from_row;
use crate::refs::display_refs_for_tasks;
use crate::task_enrichment::load_task_enrichment;
use crate::types::Task;
use crate::workspaces::active_workspace_id;

use super::TaskListItem;

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
const NOTE_WEIGHT: i64 = 80;
const FIELD_MATCH_BONUS: i64 = 35_000;
const EXTRA_FIELD_BONUS: i64 = 18_000;
const FIELD_SCORE_DIVISOR: i64 = 5;
const PRIORITY_BOOST_CAP: i64 = 18_000;
const RECENCY_BOOST_CAP: i64 = 12_000;

#[derive(Debug, Clone)]
pub(crate) struct TaskSearchQuery {
    pub(crate) text: String,
    pub(crate) include_deleted: bool,
    pub(crate) limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SearchMatchedField {
    Ref,
    Title,
    Label,
    Project,
    Status,
    Priority,
    Description,
    Note,
}

impl SearchMatchedField {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ref => "ref",
            Self::Title => "title",
            Self::Label => "label",
            Self::Project => "project",
            Self::Status => "status",
            Self::Priority => "priority",
            Self::Description => "description",
            Self::Note => "note",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TaskSearchResult {
    pub(crate) item: TaskListItem,
    pub(crate) score: i64,
    pub(crate) matched_field: SearchMatchedField,
    pub(crate) snippet: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct TaskSearchResultSet {
    pub(crate) items: Vec<TaskSearchResult>,
}

#[derive(Debug, Clone)]
pub(crate) struct TaskSearchPreviewResult {
    pub(crate) task_id: String,
    pub(crate) display_ref: String,
    pub(crate) title: String,
    pub(crate) project_key: String,
    pub(crate) status: String,
    pub(crate) priority: String,
    pub(crate) created_at: String,
    pub(crate) labels: Vec<String>,
    pub(crate) deleted: bool,
    pub(crate) score: i64,
    pub(crate) matched_field: SearchMatchedField,
    pub(crate) snippet: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct TaskSearchPreviewResultSet {
    pub(crate) items: Vec<TaskSearchPreviewResult>,
    pub(crate) total_matches: usize,
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

pub(crate) async fn search_task_items(
    conn: &mut SqliteConnection,
    query: TaskSearchQuery,
) -> Result<Vec<TaskSearchResult>> {
    let workspace_id = active_workspace_id();
    Ok(
        search_task_item_set_in_workspace(conn, &workspace_id, query)
            .await?
            .items,
    )
}

pub(crate) async fn search_task_items_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    query: TaskSearchQuery,
) -> Result<Vec<TaskSearchResult>> {
    Ok(search_task_item_set_in_workspace(conn, workspace_id, query)
        .await?
        .items)
}

pub(crate) async fn search_task_item_set_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    query: TaskSearchQuery,
) -> Result<TaskSearchResultSet> {
    let scored = scored_search_documents(conn, workspace_id, &query).await?;
    let task_ids = scored
        .items
        .iter()
        .map(|scored| scored.document.task.id.clone())
        .collect::<Vec<_>>();
    let mut enrichment = load_task_enrichment(conn, workspace_id, &task_ids).await?;
    let now_seconds = crate::queue::now_seconds();
    let items = scored
        .items
        .into_iter()
        .map(|scored| {
            let task = scored.document.task;
            let labels = enrichment
                .labels_by_task
                .remove(&task.id)
                .unwrap_or_default();
            let notes = enrichment
                .notes_by_task
                .remove(&task.id)
                .unwrap_or_default();
            let has_conflict = enrichment.conflicted_task_ids.contains(&task.id);
            let unresolved_blocker_count = *enrichment
                .unresolved_blocker_counts_by_task
                .get(&task.id)
                .unwrap_or(&0);
            let dependent_count = *enrichment
                .dependent_counts_by_task
                .get(&task.id)
                .unwrap_or(&0);
            let queue = crate::queue::queue_meta(
                &task,
                has_conflict,
                unresolved_blocker_count > 0,
                now_seconds,
            );
            TaskSearchResult {
                item: TaskListItem {
                    display_ref: scored.document.display_ref,
                    labels,
                    notes,
                    has_conflict,
                    unresolved_blocker_count,
                    dependent_count,
                    depends_on: enrichment
                        .depends_on_by_task
                        .remove(&task.id)
                        .unwrap_or_default(),
                    blocks: enrichment
                        .blocks_by_task
                        .remove(&task.id)
                        .unwrap_or_default(),
                    queue,
                    task,
                },
                score: scored.score,
                matched_field: scored.matched_field,
                snippet: scored.snippet,
            }
        })
        .collect();
    Ok(TaskSearchResultSet { items })
}

async fn scored_search_documents(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    query: &TaskSearchQuery,
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
        load_candidate_search_documents(conn, workspace_id, load_deleted, &parsed).await?;

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

pub(crate) async fn search_task_preview_set_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    query: TaskSearchQuery,
) -> Result<TaskSearchPreviewResultSet> {
    let scored = scored_search_documents(conn, workspace_id, &query).await?;
    let task_ids = scored
        .items
        .iter()
        .map(|scored| scored.document.task.id.clone())
        .collect::<Vec<_>>();
    let mut labels_by_task = labels_for_search_preview(conn, workspace_id, &task_ids).await?;
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

async fn labels_for_search_preview(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_ids: &[String],
) -> Result<HashMap<String, Vec<String>>> {
    let mut labels_by_task = HashMap::new();
    if task_ids.is_empty() {
        return Ok(labels_by_task);
    }
    for chunk in task_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        if chunk.is_empty() {
            continue;
        }
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT task_id, label FROM task_labels WHERE workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND task_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(") ORDER BY task_id, label");

        for row in query.build().fetch_all(&mut *conn).await? {
            let task_id: String = row.get("task_id");
            let label: String = row.get("label");
            labels_by_task
                .entry(task_id)
                .or_insert_with(Vec::new)
                .push(label);
        }
    }
    Ok(labels_by_task)
}

async fn load_candidate_search_documents(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    include_deleted: bool,
    parsed: &parser::ParsedTaskSearchQuery,
) -> Result<Vec<SearchDocument>> {
    let mut documents = if let Some(fts_match) = parsed.fts_match.as_deref() {
        load_fts_search_documents(conn, workspace_id, include_deleted, fts_match).await?
    } else {
        Vec::new()
    };
    if let Some(ref_query) = &parsed.ref_query {
        let ref_documents =
            load_ref_search_documents(conn, workspace_id, include_deleted, ref_query).await?;
        merge_search_documents(&mut documents, ref_documents);
    }
    Ok(documents)
}

async fn load_ref_search_documents(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    include_deleted: bool,
    ref_query: &parser::ParsedRefSearchQuery,
) -> Result<Vec<SearchDocument>> {
    let rows = sqlx::query(
        "SELECT t.id, t.workspace_id, t.title, t.description, t.project_id,
         p.key AS project_key, p.name AS project_name, p.prefix AS project_prefix,
         t.status, t.priority, t.created_at, t.updated_at, t.queue_activity_at, t.deleted,
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
    search_documents_from_rows(conn, rows).await
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
    workspace_id: &str,
    include_deleted: bool,
    raw_fts_match: &str,
) -> Result<Vec<SearchDocument>> {
    let fts_match = workspace_scoped_fts_match(workspace_id, raw_fts_match);
    let rows = sqlx::query(
        "SELECT t.id, t.workspace_id, t.title, t.description, t.project_id,
         p.key AS project_key, p.name AS project_name, p.prefix AS project_prefix,
         t.status, t.priority, t.created_at, t.updated_at, t.queue_activity_at, t.deleted,
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
    search_documents_from_rows(conn, rows).await
}

async fn search_documents_from_rows(
    conn: &mut SqliteConnection,
    rows: Vec<sqlx::sqlite::SqliteRow>,
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
    let display_refs = display_refs_for_tasks(conn, &tasks).await?;
    Ok(tasks
        .into_iter()
        .zip(project_names)
        .zip(labels_texts)
        .zip(notes_texts)
        .map(|(((task, project_name), labels_text), notes_text)| {
            let display_ref = display_refs
                .get(&task.id)
                .cloned()
                .unwrap_or_else(|| format!("{}-{}", task.project_prefix, task.id));
            SearchDocument {
                labels_text,
                notes_text,
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

fn workspace_scoped_fts_match(workspace_id: &str, fts_match: &str) -> String {
    format!("workspace_token:{} {}", fts_phrase(workspace_id), fts_match)
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
mod tests {
    use super::*;

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
}
