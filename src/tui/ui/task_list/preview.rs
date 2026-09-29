use super::super::task_display::{
    description_or_placeholder, labels_display, linked_task_ref_spans,
};
use super::super::timestamps::local_timestamp_display;
use super::EPIC_MARKER;
use super::cells::is_deferred;
use crate::query::TaskListItem;
use crate::queue::now_seconds;
use crate::tui::markdown::render_markdown_preview;
use crate::tui::store::TuiStore;
use crate::tui::text::truncate_width;
use crate::tui::theme::{self, BG};
use crate::tui::widgets::{priority_short, status_span};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};

fn task_heading_line(item: &TaskListItem) -> Line<'static> {
    let title_style = if item.task.deleted {
        Style::new()
            .fg(theme::fg_muted())
            .add_modifier(Modifier::BOLD | Modifier::CROSSED_OUT)
    } else {
        Style::new().fg(theme::fg()).add_modifier(Modifier::BOLD)
    };
    Line::from(vec![
        Span::styled(
            item.display_ref.clone(),
            Style::new()
                .fg(theme::accent())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(item.task.title.clone(), title_style),
    ])
}

pub(super) fn task_preview_fields_line(item: &TaskListItem) -> Line<'static> {
    let mut fields = vec![
        Span::styled("project ", Style::new().fg(theme::fg_dim())),
        Span::styled(
            item.task.project_key.clone(),
            Style::new()
                .fg(theme::project_color(&item.task.project_key))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  status ", Style::new().fg(theme::fg_dim())),
        status_span(item.task.status.as_str()),
        Span::styled("  priority ", Style::new().fg(theme::fg_dim())),
        Span::styled(
            priority_short(item.task.priority.as_str()),
            theme::priority_style(item.task.priority.as_str()).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  created ", Style::new().fg(theme::fg_dim())),
        Span::styled(
            local_timestamp_display(&item.task.created_at),
            Style::new().fg(theme::fg_muted()),
        ),
    ];
    if item.task.deleted {
        fields.extend([
            Span::styled("  deleted ", Style::new().fg(theme::fg_dim())),
            Span::styled(
                "yes",
                Style::new().fg(theme::red()).add_modifier(Modifier::BOLD),
            ),
        ]);
    }
    Line::from(fields)
}

fn availability_preview_line(
    item: &TaskListItem,
    now_seconds: i64,
    width: usize,
) -> Option<Line<'static>> {
    if !is_deferred(item, now_seconds) {
        return None;
    }
    let [relative, local] = crate::tui::time::availability_summary_lines(
        item.task.available_at.as_deref().unwrap_or(""),
        false,
        now_seconds,
    )?;
    let countdown = relative.strip_prefix("available ").unwrap_or(&relative);
    let fixed_width = "available ".len() + countdown.len() + " · ".len();
    let local = truncate_width(&local, width.saturating_sub(fixed_width));

    Some(Line::from(vec![
        Span::styled("available ", Style::new().fg(theme::fg_dim())),
        Span::styled(
            countdown.to_string(),
            Style::new()
                .fg(theme::accent())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::new().fg(theme::fg_dim())),
        Span::styled(local, Style::new().fg(theme::fg_muted())),
    ]))
}

fn due_preview_line(item: &TaskListItem, now_seconds: i64, width: usize) -> Option<Line<'static>> {
    let due_on = item.task.due_on.as_deref().unwrap_or("");
    let [relative, date] = crate::tui::time::due_summary_lines(due_on, now_seconds)?;
    let relative = relative.strip_prefix("due ").unwrap_or(&relative);
    let fixed_width = "due ".len() + relative.len() + " · ".len();
    let date = truncate_width(&date, width.saturating_sub(fixed_width));
    let color = if !item.task.status.is_open() {
        theme::fg_muted()
    } else {
        match crate::tui::time::due_state_at(due_on, now_seconds) {
            crate::due::DueState::Overdue(_) => theme::red(),
            crate::due::DueState::Today => theme::yellow(),
            crate::due::DueState::Future(_) => theme::accent(),
            crate::due::DueState::None => theme::fg_muted(),
        }
    };

    Some(Line::from(vec![
        Span::styled("due ", Style::new().fg(theme::fg_dim())),
        Span::styled(
            relative.to_string(),
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::new().fg(theme::fg_dim())),
        Span::styled(date, Style::new().fg(theme::fg_muted())),
    ]))
}

fn timing_preview_lines(item: &TaskListItem, now_seconds: i64, width: usize) -> Vec<Line<'static>> {
    [
        availability_preview_line(item, now_seconds, width),
        due_preview_line(item, now_seconds, width),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn epic_rollup_preview_lines(rollup: &crate::query::EpicRollup) -> Vec<Line<'static>> {
    let progress = if rollup.total == 0 {
        Line::from(vec![
            Span::styled("children ", Style::new().fg(theme::fg_dim())),
            Span::styled("none", Style::new().fg(theme::yellow())),
        ])
    } else {
        Line::from(vec![
            Span::styled("children ", Style::new().fg(theme::fg_dim())),
            Span::styled(
                format!("{} open", rollup.open),
                Style::new().fg(theme::fg()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" · {} done", rollup.done),
                Style::new().fg(theme::accent()),
            ),
            Span::styled(
                format!(" · {} canceled", rollup.canceled),
                Style::new().fg(theme::fg_muted()),
            ),
        ])
    };
    let mut lines = vec![progress];
    if rollup.total > 0 {
        lines.push(Line::from(vec![
            Span::styled("signals ", Style::new().fg(theme::fg_dim())),
            Span::styled(
                format!("{} overdue", rollup.overdue),
                Style::new().fg(if rollup.overdue > 0 {
                    theme::red()
                } else {
                    theme::fg_muted()
                }),
            ),
            Span::styled(
                format!(" · {} blocked", rollup.blocked),
                Style::new().fg(if rollup.blocked > 0 {
                    theme::yellow()
                } else {
                    theme::fg_muted()
                }),
            ),
            Span::styled(
                format!(" · {} ready", rollup.ready),
                Style::new().fg(if rollup.ready > 0 {
                    theme::accent()
                } else {
                    theme::fg_muted()
                }),
            ),
            Span::styled(" · activity ", Style::new().fg(theme::fg_dim())),
            Span::styled(
                local_timestamp_display(&rollup.latest_activity_at),
                Style::new().fg(theme::fg_muted()),
            ),
        ]));
    }
    lines
}

fn dependency_preview_lines(item: &TaskListItem) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if !item.depends_on.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("blocked by ", Style::new().fg(theme::fg_dim())),
            dependency_links_summary(&item.depends_on),
        ]));
    }
    if !item.blocks.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("blocks ", Style::new().fg(theme::fg_dim())),
            dependency_links_summary(&item.blocks),
        ]));
    }
    lines
}

fn dependency_links_summary(links: &[crate::query::TaskDependencyLink]) -> Span<'static> {
    let summary = links
        .iter()
        .take(3)
        .map(|link| format!("{} {}", link.display_ref, link.title))
        .collect::<Vec<_>>()
        .join(", ");
    let more = links.len().saturating_sub(3);
    let summary = if more > 0 {
        format!("{summary}, +{more}")
    } else {
        summary
    };
    Span::styled(summary, Style::new().fg(theme::fg_muted()))
}

pub(crate) fn render_task_preview(
    frame: &mut Frame,
    store: &TuiStore,
    selected: Option<usize>,
    area: Rect,
) {
    let Some(item) = store.selected_task(selected) else {
        return;
    };
    let block = Block::new()
        .title(" SELECTED ")
        .borders(Borders::TOP)
        .border_style(Style::new().fg(theme::border()))
        .padding(Padding::horizontal(1))
        .style(Style::new().bg(BG));
    let inner = block.inner(area);
    let lines = task_preview_lines(item, inner.width as usize, inner.height as usize);

    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::new().fg(theme::fg()).bg(BG)),
        inner,
    );
}

pub(super) fn task_preview_lines(
    item: &TaskListItem,
    width: usize,
    height: usize,
) -> Vec<Line<'static>> {
    let labels = labels_display(&item.labels, ", ");
    let mut lines = vec![task_heading_line(item), task_preview_fields_line(item)];
    lines.extend(timing_preview_lines(item, now_seconds(), width));
    if let Some(recurrence) = item.recurrence.as_ref() {
        lines.push(Line::from(vec![
            Span::styled("↻ ", Style::new().fg(theme::accent())),
            Span::styled(
                recurrence.series_ref.clone(),
                Style::new()
                    .fg(theme::accent())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    " · slot {} · {} · {}",
                    recurrence.slot_on,
                    recurrence.rule_label,
                    recurrence.lifecycle.as_str()
                ),
                Style::new().fg(theme::fg_muted()),
            ),
        ]));
    }
    if let Some(group) = item.recurrence_group.as_ref() {
        lines.push(Line::from(Span::styled(
            format!(
                "series history ✓{} completed · ↷{} skipped · ×{} missed",
                group.counts.completed, group.counts.skipped, group.counts.missed
            ),
            Style::new().fg(theme::fg_muted()),
        )));
    }
    lines.push(Line::from(vec![
        Span::styled("labels ", Style::new().fg(theme::fg_dim())),
        Span::styled(labels, Style::new().fg(theme::fg_muted())),
    ]));
    lines.extend(dependency_preview_lines(item));
    if let Some(rollup) = item.epic_rollup.as_ref() {
        lines.extend(epic_rollup_preview_lines(rollup));
    }
    if let Some(parent) = &item.epic_parent {
        lines.push(epic_parent_preview_line(parent));
    }
    let open_child_links: Vec<_> = item
        .epic_children
        .iter()
        .filter(|link| link.unresolved)
        .collect();
    if !open_child_links.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(
                "CHILD TASKS ",
                Style::new()
                    .fg(theme::fg_dim())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "({} open of {})",
                    open_child_links.len(),
                    item.epic_children.len()
                ),
                Style::new().fg(theme::accent()),
            ),
        ]));
        let last_child_index = open_child_links.len().saturating_sub(1);
        for (index, link) in open_child_links.iter().take(5).enumerate() {
            let branch = if index == last_child_index {
                "└─"
            } else {
                "├─"
            };
            let mut spans = vec![Span::styled(
                format!("  {branch} "),
                Style::new().fg(theme::fg_dim()),
            )];
            spans.extend(linked_task_ref_spans(&link.display_ref, &link.project_key));
            spans.extend([
                Span::raw(" "),
                Span::styled(link.title.clone(), Style::new().fg(theme::fg_muted())),
                Span::styled(
                    format!(" {}", link.status),
                    Style::new().fg(theme::fg_dim()),
                ),
            ]);
            lines.push(Line::from(spans));
        }
        if open_child_links.len() > 5 {
            lines.push(Line::from(vec![Span::styled(
                format!("  ... +{} more", open_child_links.len() - 5),
                Style::new().fg(theme::fg_dim()),
            )]));
        }
    }

    if lines.len() < height {
        if height - lines.len() > 1 {
            lines.push(Line::from(""));
        }
        let description_height = height.saturating_sub(lines.len());
        lines.extend(render_markdown_preview(
            &description_or_placeholder(&item.task.description),
            width,
            description_height,
        ));
    }
    lines.truncate(height);
    lines
}

fn epic_parent_preview_line(parent: &crate::query::TaskDependencyLink) -> Line<'static> {
    Line::from(vec![
        Span::styled("part of ", Style::new().fg(theme::fg_dim())),
        Span::styled(EPIC_MARKER, Style::new().fg(theme::yellow())),
        Span::styled(" ", Style::new().fg(theme::fg_dim())),
        Span::styled(
            format!("{} {}", parent.display_ref, parent.title),
            Style::new().fg(theme::fg_muted()),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::test_support::task_list_item;
    use chrono::TimeZone;

    #[test]
    fn task_preview_fields_show_created_timestamp() {
        let item = task_list_item("preview");
        let rendered = task_preview_fields_line(&item).to_string();

        assert!(rendered.contains("created "));
    }

    #[test]
    fn task_preview_shows_future_availability() {
        let mut item = task_list_item("preview");
        item.task.available_at = Some("200".to_string());

        let line = availability_preview_line(&item, 100, 80).unwrap();

        assert!(line.to_string().starts_with("available in 1m · "));
        assert_eq!(line.spans[0].style.fg, Some(theme::fg_dim()));
        assert_eq!(line.spans[1].style.fg, Some(theme::accent()));
        assert!(line.spans[1].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(line.spans[3].style.fg, Some(theme::fg_muted()));
    }

    #[test]
    fn task_preview_omits_elapsed_availability() {
        let mut item = task_list_item("preview");
        item.task.available_at = Some("100".to_string());

        assert!(availability_preview_line(&item, 200, 80).is_none());
    }

    #[test]
    fn task_preview_shows_availability_and_due_date_responsively() {
        let now = chrono::Local
            .with_ymd_and_hms(2026, 7, 16, 12, 0, 0)
            .single()
            .unwrap()
            .timestamp();
        let mut item = task_list_item("preview");
        item.task.available_at = Some((now + 3_600).to_string());
        item.task.due_on = Some("2026-07-17".to_string());

        let lines = timing_preview_lines(&item, now, 24);

        assert_eq!(lines.len(), 2);
        assert!(lines[0].to_string().starts_with("available in 1h · "));
        assert!(lines[1].to_string().starts_with("due tomorrow · "));
        assert!(lines.iter().all(|line| line.width() <= 24));
        assert_eq!(lines[1].spans[0].style.fg, Some(theme::fg_dim()));
        assert_eq!(lines[1].spans[1].style.fg, Some(theme::accent()));
        assert!(
            lines[1].spans[1]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert_eq!(lines[1].spans[3].style.fg, Some(theme::fg_muted()));
    }

    #[test]
    fn task_preview_timing_lines_preserve_absent_values() {
        let now = chrono::Local
            .with_ymd_and_hms(2026, 7, 16, 12, 0, 0)
            .single()
            .unwrap()
            .timestamp();
        let mut item = task_list_item("preview");

        assert!(timing_preview_lines(&item, now, 80).is_empty());

        item.task.available_at = Some((now + 3_600).to_string());
        assert_eq!(timing_preview_lines(&item, now, 80).len(), 1);
        assert!(
            timing_preview_lines(&item, now, 80)[0]
                .to_string()
                .starts_with("available ")
        );

        item.task.available_at = None;
        item.task.due_on = Some("2026-07-17".to_string());
        assert_eq!(timing_preview_lines(&item, now, 80).len(), 1);
        assert!(
            timing_preview_lines(&item, now, 80)[0]
                .to_string()
                .starts_with("due ")
        );
    }

    #[test]
    fn preview_marks_epic_parent_with_star() {
        let parent = crate::query::TaskDependencyLink {
            project_key: "app".to_string(),
            task_id: crate::test_support::task_id("parent-task-id"),
            display_ref: "APP-EPIC".to_string(),
            title: "Build the epic container".to_string(),
            status: "inbox".to_string(),
            priority: "medium".to_string(),
            unresolved: true,
        };

        let line = epic_parent_preview_line(&parent);

        assert_eq!(
            line.to_string(),
            format!("part of {EPIC_MARKER} APP-EPIC Build the epic container")
        );
    }

    #[test]
    fn preview_renders_markdown_blocks_and_inline_styles() {
        let mut item = task_list_item("documented");
        item.task.description =
            "### Context\n\nFirst **bold** paragraph.\n\n- one\n- `two`".to_string();

        let lines = task_preview_lines(&item, 40, 12);
        let description = &lines[4..];
        let rendered = description
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(rendered, "Context\n\nFirst bold paragraph.\n\n- one\n- two");
        assert!(!rendered.contains("###"));
        assert!(
            description[2]
                .spans
                .iter()
                .any(|span| span.content == "bold"
                    && span.style.add_modifier.contains(Modifier::BOLD))
        );
    }

    #[test]
    fn preview_bounds_wrapped_markdown_to_available_height() {
        let mut item = task_list_item("documented");
        item.task.description = "A description with enough words to wrap across many lines in the selected task preview.".to_string();

        let lines = task_preview_lines(&item, 20, 7);

        assert_eq!(lines.len(), 7);
        assert_eq!(lines[3].to_string(), "");
        assert!(lines[6].to_string().ends_with('…'));
        assert!(lines[4..].iter().all(|line| line.width() <= 20));
    }

    #[test]
    fn preview_uses_single_remaining_line_for_description() {
        let mut item = task_list_item("documented");
        item.task.description = "small body".to_string();

        let lines = task_preview_lines(&item, 20, 4);

        assert_eq!(lines.len(), 4);
        assert_eq!(lines[3].to_string(), "small body");
    }

    #[test]
    fn preview_shows_child_tasks_for_epic_parent() {
        let mut item = task_list_item("epic");
        item.epic_children = vec![
            crate::query::TaskDependencyLink {
                project_key: "app".to_string(),
                task_id: crate::test_support::task_id("child-1"),
                display_ref: "APP-C001".to_string(),
                title: "first child".to_string(),
                status: "todo".to_string(),
                priority: "none".to_string(),
                unresolved: true,
            },
            crate::query::TaskDependencyLink {
                project_key: "app".to_string(),
                task_id: crate::test_support::task_id("child-2"),
                display_ref: "APP-C002".to_string(),
                title: "second child".to_string(),
                status: "active".to_string(),
                priority: "none".to_string(),
                unresolved: true,
            },
        ];

        let rendered = task_preview_lines(&item, 80, 20)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("CHILD TASKS"));
        assert!(rendered.contains("(2 open of 2)"));
        assert!(rendered.contains("  ├─ APP-C001"));
        assert!(rendered.contains("first child"));
        assert!(rendered.contains("  └─ APP-C002"));
        assert!(rendered.contains("second child"));
    }
}
