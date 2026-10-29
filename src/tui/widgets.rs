use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::query::TaskListItem;
use crate::queue::unix_seconds;
use crate::tui::text::truncate_width;
use crate::tui::theme::Theme;

/// Icon and short label for each priority.
///
/// Low, medium, and high share an ascending three-bar silhouette with one, two,
/// or three bars filled, so the level reads at a glance. Urgent uses a filled
/// exclamation box rather than a fourth bar. The bar and box glyphs are Nerd
/// Font Material Design icons and occupy one cell.
fn priority_parts(priority: &str) -> (&'static str, &'static str) {
    match priority {
        // md-alert_box
        "urgent" => ("\u{f0027}", "urgent"),
        // md-signal_cellular_3
        "high" => ("\u{f08be}", "high"),
        // md-signal_cellular_2
        "medium" => ("\u{f08bd}", "med"),
        // md-signal_cellular_1
        "low" => ("\u{f08bc}", "low"),
        _ => ("─", "none"),
    }
}

pub(crate) fn priority_icon(priority: &str) -> &'static str {
    priority_parts(priority).0
}

pub(crate) fn priority_short(priority: &str) -> String {
    let (icon, label) = priority_parts(priority);
    format!("{icon} {label}")
}

/// Icon and label for each status.
///
/// The icon is the compact rendering of the status and must stay distinct for
/// every status so an icon-only column remains unambiguous.
fn status_parts(status: &str) -> Option<(&'static str, &'static str)> {
    match status {
        "active" => Some(("●", "active")),
        "todo" => Some(("□", "todo")),
        "inbox" => Some(("▣", "inbox")),
        "backlog" => Some(("◌", "back")),
        "done" => Some(("✓", "done")),
        "canceled" => Some(("×", "cancel")),
        _ => None,
    }
}

pub(crate) fn status_chip(theme: &Theme, status: &str) -> Line<'static> {
    Line::from(status_span(theme, status))
}

/// Single-glyph status cell used by the compact status column.
///
/// The glyph is the same one the textual status cell starts with, so compact
/// and textual renderings stay recognizable as the same status.
pub(crate) fn status_icon_cell(theme: &Theme, status: &str) -> Line<'static> {
    Line::from(status_icon_span(theme, status))
}

pub(crate) fn status_icon_span(theme: &Theme, status: &str) -> Span<'static> {
    status_span_inner(theme, status, true)
}

pub(crate) fn status_span(theme: &Theme, status: &str) -> Span<'static> {
    status_span_inner(theme, status, false)
}

/// Text of `status_span`, for sizing without styling.
pub(crate) fn status_text(status: &str) -> String {
    status_label(status, false)
}

fn status_label(status: &str, icons_only: bool) -> String {
    match status_parts(status) {
        Some((icon, _)) if icons_only => icon.to_string(),
        Some((icon, text)) => format!("{icon} {text}"),
        None => status.to_string(),
    }
}

fn status_span_inner(theme: &Theme, status: &str, icons_only: bool) -> Span<'static> {
    let label = status_label(status, icons_only);
    Span::styled(label, theme.status(status).add_modifier(Modifier::BOLD))
}

pub(crate) fn age_style(theme: &Theme, created_at: &str, now_seconds: i64) -> Style {
    let days = unix_seconds(created_at)
        .map(|created| {
            now_seconds
                .saturating_sub(created)
                .max(0)
                .saturating_div(86_400)
        })
        .unwrap_or(0);
    let color = if days < 1 {
        theme.success
    } else if days < 4 {
        theme.fg_muted
    } else if days < 8 {
        theme.fg
    } else if days < 15 {
        theme.warning
    } else {
        theme.danger
    };
    Style::new().fg(color)
}

pub(crate) fn title_cell(theme: &Theme, item: &TaskListItem, max_width: usize) -> Line<'static> {
    let mut marker = String::new();
    if item.has_conflict {
        marker.push_str("⚡ ");
    }
    if item.recurrence.is_some() {
        marker.push_str("↻ ");
    }
    let content_width = max_width.saturating_sub(1);
    let title_style = if item.task.deleted {
        theme.muted().add_modifier(Modifier::CROSSED_OUT)
    } else {
        Style::new().fg(theme.fg)
    };
    let marker_width = marker.width();
    let title_width = content_width.saturating_sub(marker_width);
    let title_text = if let Some(group) = item.recurrence_group.as_ref() {
        format!(
            "{} · {} {} ✓{} ↷{} ×{}",
            item.task.title,
            group.series_ref,
            group.counts.latest_slot_on.as_deref().unwrap_or(""),
            group.counts.completed,
            group.counts.skipped,
            group.counts.missed
        )
    } else if let Some(recurrence) = item.recurrence.as_ref() {
        format!("{} · {}", item.task.title, recurrence.slot_on)
    } else {
        item.task.title.clone()
    };
    let title = truncate_width(&title_text, title_width);
    Line::from(vec![
        Span::styled(marker, Style::new().fg(theme.warning)),
        Span::styled(title, title_style),
    ])
}

pub(crate) fn label_cell(theme: &Theme, labels: &[String], max_width: usize) -> Line<'static> {
    if max_width == 0 {
        return Line::from("");
    }
    let text = label_summary_text(labels);
    let text_width = text.width();
    if text.is_empty() || text_width > max_width {
        return Line::from("");
    }
    let trailing_gap = usize::from(text_width < max_width);
    let padding = max_width.saturating_sub(text_width + trailing_gap);
    let mut spans = vec![
        Span::raw(" ".repeat(padding)),
        Span::styled(text, theme.dim()),
    ];
    if trailing_gap > 0 {
        spans.push(Span::raw(" "));
    }
    Line::from(spans)
}

fn label_summary_text(labels: &[String]) -> String {
    let Some(first) = labels.first() else {
        return String::new();
    };
    let more = labels.len().saturating_sub(1);
    if more == 0 {
        first.clone()
    } else {
        format!("{first} +{more}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::test_support::task_list_item;

    const THEME: &Theme = &Theme::dark();

    #[test]
    fn status_icons_cover_every_status_distinctly() {
        let mut icons = std::collections::BTreeSet::new();

        for status in crate::choices::STATUSES {
            let icon = status_icon_span(THEME, status).content.to_string();

            assert_eq!(icon.chars().count(), 1, "{status}");
            assert!(icons.insert(icon.clone()), "{status} reuses {icon}");
            assert!(
                status_span(THEME, status).content.starts_with(&icon),
                "{status} text does not start with {icon}"
            );
        }

        assert_eq!(icons.len(), crate::choices::STATUSES.len());
    }

    #[test]
    fn priority_icons_use_signal_bars_and_urgent_box() {
        assert_eq!(priority_icon("none"), "─");
        assert_eq!(priority_icon("low"), "\u{f08bc}");
        assert_eq!(priority_icon("medium"), "\u{f08bd}");
        assert_eq!(priority_icon("high"), "\u{f08be}");
        assert_eq!(priority_icon("urgent"), "\u{f0027}");

        let mut icons = std::collections::BTreeSet::new();
        for priority in crate::choices::PRIORITIES {
            let icon = priority_icon(priority);
            assert_eq!(icon.width(), 1, "{priority}");
            assert!(icons.insert(icon), "{priority} reuses {icon}");
            assert!(
                status_parts_icons().all(|status_icon| status_icon != icon),
                "{priority} reuses a status icon"
            );
            assert_eq!(
                priority_short(priority),
                format!(
                    "{icon} {}",
                    if *priority == "medium" {
                        "med"
                    } else {
                        priority
                    }
                )
            );
        }
    }

    fn status_parts_icons() -> impl Iterator<Item = &'static str> {
        crate::choices::STATUSES
            .iter()
            .filter_map(|status| status_parts(status).map(|(icon, _)| icon))
    }

    #[test]
    fn status_icon_cell_uses_status_style() {
        let cell = status_icon_cell(THEME, "done");

        assert_eq!(cell.to_string(), "✓");
        assert_eq!(cell.spans[0].style.fg, THEME.status("done").fg);
        assert!(cell.spans[0].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn label_cell_right_aligns_summary_when_space_allows() {
        let mut item = task_list_item("Short title");
        item.labels = vec!["search".to_string(), "ux".to_string()];

        let rendered = label_cell(THEME, &item.labels, 11).to_string();

        assert_eq!(rendered, " search +1 ");
        assert_eq!(rendered.chars().count(), 11);
    }

    #[test]
    fn label_cell_uses_dim_text() {
        let mut item = task_list_item("Short title");
        item.labels = vec!["search".to_string()];

        let line = label_cell(THEME, &item.labels, 6);
        let rendered = line.to_string();

        assert_eq!(rendered, "search");
        let label = line
            .spans
            .iter()
            .find(|span| span.content == "search")
            .unwrap();
        assert_eq!(label.style.fg, Some(THEME.fg_dim));
        assert_eq!(label.style.bg, None);
        assert!(!label.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn label_cell_hides_labels_when_space_is_tight() {
        let mut item = task_list_item("Short title");
        item.labels = vec!["search".to_string()];

        let rendered = label_cell(THEME, &item.labels, 5).to_string();

        assert_eq!(rendered, "");
    }

    #[test]
    fn title_cell_truncates_title_when_labels_are_hidden() {
        let mut item = task_list_item("A very long task title");
        item.labels = vec!["search".to_string()];

        let rendered = title_cell(THEME, &item, 12).to_string();

        assert_eq!(rendered, "A very lon…");
    }

    #[test]
    fn title_cell_truncates_wide_titles_by_cells() {
        let item = task_list_item("한글입력");
        let line = title_cell(THEME, &item, 6);

        assert_eq!(line.to_string(), "한글…");
        assert!(line.width() <= 6);
    }

    #[test]
    fn label_cell_sizes_wide_labels_by_cells() {
        let labels = vec!["한글".to_string()];
        let line = label_cell(THEME, &labels, 4);

        assert_eq!(line.to_string(), "한글");
        assert_eq!(line.width(), 4);
    }
}
