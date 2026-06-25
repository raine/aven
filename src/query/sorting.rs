use sqlx::{QueryBuilder, Sqlite};

use super::{SortDirection, TaskSort};

pub(super) fn push_sort(
    query: &mut QueryBuilder<Sqlite>,
    sort: TaskSort,
    direction: SortDirection,
) {
    match (sort, direction) {
        (TaskSort::Queue, _) => query.push(" ORDER BY t.created_at ASC"),
        (TaskSort::Created, SortDirection::Asc) => query.push(" ORDER BY t.created_at ASC"),
        (TaskSort::Created, SortDirection::Desc) => query.push(" ORDER BY t.created_at DESC"),
        (TaskSort::Updated, SortDirection::Asc) => {
            query.push(" ORDER BY t.updated_at ASC, t.created_at ASC, t.rowid ASC")
        }
        (TaskSort::Updated, SortDirection::Desc) => {
            query.push(" ORDER BY t.updated_at DESC, t.created_at DESC, t.rowid ASC")
        }
        (TaskSort::Priority, SortDirection::Asc) => query.push(
            " ORDER BY
              CASE t.priority
                WHEN 'urgent' THEN 0
                WHEN 'high' THEN 1
                WHEN 'medium' THEN 2
                WHEN 'low' THEN 3
                WHEN 'none' THEN 4
                ELSE 5
              END,
              t.created_at DESC",
        ),
        (TaskSort::Priority, SortDirection::Desc) => query.push(
            " ORDER BY
              CASE t.priority
                WHEN 'none' THEN 0
                WHEN 'low' THEN 1
                WHEN 'medium' THEN 2
                WHEN 'high' THEN 3
                WHEN 'urgent' THEN 4
                ELSE 5
              END,
              t.created_at DESC",
        ),
        (TaskSort::Project, SortDirection::Asc) => {
            query.push(" ORDER BY p.key ASC, t.created_at DESC")
        }
        (TaskSort::Project, SortDirection::Desc) => {
            query.push(" ORDER BY p.key DESC, t.created_at DESC")
        }
        (TaskSort::Title, SortDirection::Asc) => {
            query.push(" ORDER BY lower(t.title) ASC, t.created_at DESC")
        }
        (TaskSort::Title, SortDirection::Desc) => {
            query.push(" ORDER BY lower(t.title) DESC, t.created_at DESC")
        }
    };
}
