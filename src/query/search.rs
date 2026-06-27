use anyhow::Result;
use serde::Serialize;
use sqlx::{Row, SqliteConnection};

use crate::db::task_from_row;
use crate::refs::display_refs_for_tasks;
use crate::task_enrichment::load_task_enrichment;
use crate::types::Task;
use crate::workspaces::active_workspace_id;

use super::TaskListItem;

const DEFAULT_LIMIT: usize = 50;
const REF_WEIGHT: i64 = 1_000;
const TITLE_WEIGHT: i64 = 420;
const LABEL_WEIGHT: i64 = 240;
const PROJECT_WEIGHT: i64 = 220;
const STATUS_WEIGHT: i64 = 160;
const PRIORITY_WEIGHT: i64 = 150;
const DESCRIPTION_WEIGHT: i64 = 100;
const NOTE_WEIGHT: i64 = 80;

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

struct SearchDocument {
    task: Task,
    display_ref: String,
    project_name: String,
    labels: Vec<String>,
    notes: Vec<super::TaskNote>,
}

struct ScoredDocument {
    document: SearchDocument,
    score: i64,
    matched_field: SearchMatchedField,
    snippet: Option<String>,
}

pub(crate) async fn search_task_items(
    conn: &mut SqliteConnection,
    query: TaskSearchQuery,
) -> Result<Vec<TaskSearchResult>> {
    let workspace_id = active_workspace_id();
    search_task_items_in_workspace(conn, &workspace_id, query).await
}

pub(crate) async fn search_task_items_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    query: TaskSearchQuery,
) -> Result<Vec<TaskSearchResult>> {
    let limit = if query.limit == 0 {
        DEFAULT_LIMIT
    } else {
        query.limit
    };
    let documents = load_search_documents(conn, workspace_id, query.include_deleted).await?;
    let text = query.text.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }

    let mut scored = documents
        .into_iter()
        .filter_map(|document| score_document(document, text))
        .collect::<Vec<_>>();
    scored.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.document.task.updated_at.cmp(&a.document.task.updated_at))
            .then_with(|| a.document.task.title.cmp(&b.document.task.title))
            .then_with(|| a.document.task.id.cmp(&b.document.task.id))
    });
    scored.truncate(limit);

    let tasks = scored
        .iter()
        .map(|scored| scored.document.task.clone())
        .collect::<Vec<_>>();
    let task_ids = tasks.iter().map(|task| task.id.clone()).collect::<Vec<_>>();
    let mut enrichment = load_task_enrichment(conn, workspace_id, &task_ids).await?;
    let now_seconds = crate::queue::now_seconds();
    Ok(scored
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
        .collect())
}

async fn load_search_documents(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    include_deleted: bool,
) -> Result<Vec<SearchDocument>> {
    let rows = sqlx::query(
        "SELECT t.id, t.workspace_id, t.title, t.description, t.project_id,
         p.key AS project_key, p.name AS project_name, p.prefix AS project_prefix,
         t.status, t.priority, t.created_at, t.updated_at, t.queue_activity_at, t.deleted
         FROM tasks t JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
         WHERE t.workspace_id = ? AND (? OR t.deleted = 0)
         ORDER BY t.updated_at DESC, t.id",
    )
    .bind(workspace_id)
    .bind(include_deleted)
    .fetch_all(&mut *conn)
    .await?;
    let mut tasks = Vec::with_capacity(rows.len());
    let mut project_names = Vec::with_capacity(rows.len());
    for row in rows {
        project_names.push(row.get::<String, _>("project_name"));
        tasks.push(task_from_row(&row)?);
    }
    let display_refs = display_refs_for_tasks(conn, &tasks).await?;
    let task_ids = tasks.iter().map(|task| task.id.clone()).collect::<Vec<_>>();
    let mut enrichment = load_task_enrichment(conn, workspace_id, &task_ids).await?;
    Ok(tasks
        .into_iter()
        .zip(project_names)
        .map(|(task, project_name)| {
            let display_ref = display_refs
                .get(&task.id)
                .cloned()
                .unwrap_or_else(|| format!("{}-{}", task.project_prefix, task.id));
            SearchDocument {
                labels: enrichment
                    .labels_by_task
                    .remove(&task.id)
                    .unwrap_or_default(),
                notes: enrichment
                    .notes_by_task
                    .remove(&task.id)
                    .unwrap_or_default(),
                task,
                display_ref,
                project_name,
            }
        })
        .collect())
}

fn score_document(document: SearchDocument, query: &str) -> Option<ScoredDocument> {
    let ref_text = format!(
        "{} {} {}-{}",
        document.display_ref, document.task.id, document.task.project_prefix, document.task.id
    );
    let project_text = format!(
        "{} {} {}",
        document.task.project_key, document.project_name, document.task.project_prefix
    );
    let labels_text = document.labels.join(" ");
    let notes_text = document
        .notes
        .iter()
        .map(|note| note.body.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let lanes = [
        (SearchMatchedField::Ref, ref_text.as_str(), REF_WEIGHT),
        (
            SearchMatchedField::Title,
            document.task.title.as_str(),
            TITLE_WEIGHT,
        ),
        (
            SearchMatchedField::Label,
            labels_text.as_str(),
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
        (SearchMatchedField::Note, notes_text.as_str(), NOTE_WEIGHT),
    ];
    lanes
        .into_iter()
        .filter_map(|(field, text, weight)| {
            score_lane(text, query).map(|(score, span)| {
                let snippet = if matches!(
                    field,
                    SearchMatchedField::Description | SearchMatchedField::Note
                ) {
                    snippet(text, span)
                } else {
                    None
                };
                (score * weight, field, snippet)
            })
        })
        .max_by_key(|(score, _, _)| *score)
        .map(|(score, matched_field, snippet)| ScoredDocument {
            document,
            score,
            matched_field,
            snippet,
        })
}

fn score_lane(text: &str, query: &str) -> Option<(i64, std::ops::Range<usize>)> {
    let normalized_text = text.to_ascii_lowercase();
    let raw_query = query.trim();
    let normalized_query = raw_query.to_ascii_lowercase();
    let query = normalized_query.trim();
    if query.is_empty() || normalized_text.is_empty() {
        return None;
    }
    let normalized_ref_query = normalize_ref_query(query);
    if normalized_ref_query.len() >= 3 {
        let normalized_ref_text = normalize_ref_query(&normalized_text);
        if let Some(index) = normalized_ref_text.find(&normalized_ref_query) {
            return Some((2_000 - index as i64, 0..text.len().min(query.len())));
        }
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
    if query.len() < 3 || looks_like_ref_query(raw_query) {
        return None;
    }
    subsequence_span(&normalized_text, query).and_then(|span| {
        let gap = span.end.saturating_sub(span.start + query.len()) as i64;
        let boundary_bonus =
            if span.start == 0 || is_boundary(normalized_text.as_bytes()[span.start - 1]) {
                120
            } else {
                0
            };
        let score = 500 + boundary_bonus - gap * 8 - span.start as i64;
        (score > 0).then_some((score, span))
    })
}

fn subsequence_span(text: &str, query: &str) -> Option<std::ops::Range<usize>> {
    let mut query_chars = query.chars();
    let mut next = query_chars.next()?;
    let mut start = None;
    for (index, ch) in text.char_indices() {
        if ch == next {
            if start.is_none() {
                start = Some(index);
            }
            let end = index + ch.len_utf8();
            if let Some(ch) = query_chars.next() {
                next = ch;
            } else {
                return Some(start.unwrap_or(index)..end);
            }
        }
    }
    None
}

fn looks_like_ref_query(input: &str) -> bool {
    let trimmed = input.trim().trim_start_matches('/');
    let Some((prefix, suffix)) = trimmed.split_once('-') else {
        return false;
    };
    !prefix.is_empty()
        && prefix.chars().all(|ch| ch.is_ascii_alphabetic())
        && suffix.len() >= 3
        && suffix.chars().all(|ch| ch.is_ascii_alphanumeric())
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
