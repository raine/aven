use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState};

use super::truncate::truncate_chars;
use crate::choices::TaskPriority;
use crate::tui::app::{Focus, WidgetState};
use crate::tui::store::{
    SidebarEntry, SidebarEntryTarget, TaskScope, TaskScopeTarget, TaskView, TuiStore,
};
use crate::tui::theme::{
    self, ACCENT, BG, BG_ALT, BORDER, FG, FG_DIM, FG_MUTED, PINK, RED, SELECTED, SELECTED_INACTIVE,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SidebarClick {
    pub(crate) entry_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SidebarLayout {
    pub(crate) sidebar: Rect,
    pub(crate) content: Rect,
    pub(crate) overlay: bool,
}

#[cfg(test)]
pub(crate) fn sidebar_layout(terminal: Rect, focus: Focus) -> Option<SidebarLayout> {
    sidebar_layout_for(terminal, focus, true)
}

pub(crate) fn sidebar_layout_for(
    terminal: Rect,
    focus: Focus,
    sidebar_visible: bool,
) -> Option<SidebarLayout> {
    if !sidebar_visible || terminal.width < 70 || terminal.height < 18 {
        return None;
    }

    let body = Rect {
        x: terminal.x,
        y: terminal.y + 2,
        width: terminal.width,
        height: terminal.height.saturating_sub(4),
    };

    let overlay = body.width < 100;
    if overlay && focus != Focus::Sidebar {
        return None;
    }

    let sidebar = if overlay {
        sidebar_overlay_area(body)
    } else {
        Rect {
            x: body.x,
            y: body.y,
            width: body.width.min(26),
            height: body.height,
        }
    };

    Some(SidebarLayout {
        sidebar,
        content: sidebar_content_area(sidebar, overlay),
        overlay,
    })
}

pub(crate) fn sidebar_overlay_area(area: Rect) -> Rect {
    Rect {
        x: area.x + 2,
        y: area.y + 1,
        width: area.width.saturating_sub(4).min(34),
        height: area.height.saturating_sub(2).min(24),
    }
}

pub(crate) fn sidebar_content_area(sidebar: Rect, overlay: bool) -> Rect {
    if overlay {
        Rect {
            x: sidebar.x + 1,
            y: sidebar.y + 1,
            width: sidebar.width.saturating_sub(2),
            height: sidebar.height.saturating_sub(2),
        }
    } else {
        Rect {
            x: sidebar.x,
            y: sidebar.y,
            width: sidebar.width.saturating_sub(1),
            height: sidebar.height,
        }
    }
}

pub(crate) fn sidebar_click_at_for(
    entries: &[SidebarEntry],
    state: &ListState,
    focus: Focus,
    sidebar_visible: bool,
    terminal: Rect,
    column: u16,
    row: u16,
) -> Option<SidebarClick> {
    let layout = sidebar_layout_for(terminal, focus, sidebar_visible)?;
    if column < layout.content.x
        || column >= layout.content.x.saturating_add(layout.content.width)
        || row < layout.content.y
        || row >= layout.content.y.saturating_add(layout.content.height)
    {
        return None;
    }

    let entry_index = usize::from(row - layout.content.y).saturating_add(state.offset());
    entries.get(entry_index).and_then(|entry| {
        if entry.target.is_some() {
            Some(SidebarClick { entry_index })
        } else {
            None
        }
    })
}

pub(super) fn render_sidebar_overlay(
    frame: &mut Frame,
    store: &TuiStore,
    widgets: &mut WidgetState,
    focus: Focus,
    area: Rect,
) {
    let area = sidebar_overlay_area(area);
    frame.render_widget(Clear, area);
    render_sidebar(frame, store, widgets, focus, area, true);
}

pub(super) fn render_sidebar(
    frame: &mut Frame,
    store: &TuiStore,
    widgets: &mut WidgetState,
    focus: Focus,
    area: Rect,
    overlay: bool,
) {
    let content_width = area.width.saturating_sub(if overlay { 2 } else { 1 }) as usize;
    let mut items: Vec<ListItem> = store
        .sidebar_entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            if entry.section {
                if entry.label.is_empty() || entry.label == "Smart Views" {
                    return ListItem::new(Line::from(""));
                }
                return ListItem::new(
                    Line::from(format!(" {} ", entry.label.to_uppercase())).style(
                        Style::new()
                            .fg(FG_DIM)
                            .bg(BG_ALT)
                            .add_modifier(Modifier::BOLD),
                    ),
                );
            }
            let marker = if index == widgets.sidebar.selected().unwrap_or(usize::MAX) {
                "≡"
            } else {
                sidebar_icon(entry)
            };
            let label = sidebar_label(entry);
            let is_active_view = sidebar_entry_active(entry, store);
            let color = match &entry.target {
                Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(project))) => {
                    theme::project_color(project)
                }
                Some(SidebarEntryTarget::View(TaskView::Active)) => FG_MUTED,
                Some(SidebarEntryTarget::View(TaskView::Todo)) => FG_DIM,
                _ => FG,
            };
            let label_style = if is_active_view {
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(FG)
            };
            let line = sidebar_entry_line(
                entry,
                marker,
                &label,
                label_style,
                color,
                is_active_view,
                content_width,
            );
            ListItem::new(line)
        })
        .collect();

    let urgent_count = store
        .tasks
        .iter()
        .filter(|task| task.task.priority == TaskPriority::Urgent)
        .count() as i64;
    let conflict_count = store.tasks.iter().filter(|task| task.has_conflict).count() as i64;
    items.extend([
        ListItem::new(Line::from("")),
        ListItem::new(
            Line::from("FILTERS").style(Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD)),
        ),
        filter_item("▲", "urgent", urgent_count, RED, area.width),
        filter_item("⚡", "conflicts", conflict_count, PINK, area.width),
    ]);

    let highlight_style = if focus == Focus::Sidebar {
        SELECTED
    } else {
        SELECTED_INACTIVE
    };
    let borders = if overlay {
        Borders::ALL
    } else {
        Borders::RIGHT
    };
    let list = List::new(items)
        .block(
            Block::new()
                .borders(borders)
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(BORDER))
                .style(Style::new().bg(BG)),
        )
        .highlight_style(highlight_style);
    frame.render_stateful_widget(list, area, &mut widgets.sidebar);
}

fn badge(count: i64, active: bool) -> Span<'static> {
    if count <= 0 {
        return Span::raw(" ");
    }
    let color = if active { ACCENT } else { FG_MUTED };
    Span::styled(
        format!("{count:>2}"),
        Style::new().fg(color).add_modifier(Modifier::BOLD),
    )
}

fn sidebar_entry_active(entry: &SidebarEntry, store: &TuiStore) -> bool {
    match &entry.target {
        Some(SidebarEntryTarget::View(view)) => *view == store.view_state.view,
        Some(SidebarEntryTarget::Scope(TaskScopeTarget::Workspace)) => {
            store.view_state.scope == TaskScope::Workspace
        }
        Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(project))) => {
            store.scope_project() == Some(project.as_str())
        }
        None => false,
    }
}

fn sidebar_icon(entry: &SidebarEntry) -> &'static str {
    match entry.target {
        Some(SidebarEntryTarget::View(TaskView::Queue)) => "○",
        Some(SidebarEntryTarget::View(TaskView::Inbox)) => "▣",
        Some(SidebarEntryTarget::View(TaskView::Todo)) => "□",
        Some(SidebarEntryTarget::View(TaskView::Active)) => "●",
        Some(SidebarEntryTarget::View(TaskView::Backlog)) => "◌",
        Some(SidebarEntryTarget::View(TaskView::Done)) => "✓",
        Some(SidebarEntryTarget::View(TaskView::Conflicts)) => "!",
        Some(SidebarEntryTarget::View(TaskView::Search)) => "⌕",
        Some(SidebarEntryTarget::View(TaskView::Open)) => "○",
        Some(SidebarEntryTarget::Scope(TaskScopeTarget::Workspace)) => "◆",
        Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(_))) => "●",
        None => " ",
    }
}

fn sidebar_label(entry: &SidebarEntry) -> String {
    match entry.target {
        Some(SidebarEntryTarget::View(TaskView::Queue)) => "Queue".to_string(),
        Some(SidebarEntryTarget::View(TaskView::Inbox)) => "Inbox".to_string(),
        Some(SidebarEntryTarget::View(TaskView::Active)) => "All active".to_string(),
        Some(SidebarEntryTarget::View(TaskView::Backlog)) => "Backlog".to_string(),
        Some(SidebarEntryTarget::View(TaskView::Todo)) => "All todo".to_string(),
        Some(SidebarEntryTarget::View(TaskView::Done)) => "Done".to_string(),
        Some(SidebarEntryTarget::View(TaskView::Conflicts)) => "Conflicts".to_string(),
        Some(SidebarEntryTarget::View(TaskView::Search)) => "Search".to_string(),
        Some(SidebarEntryTarget::View(TaskView::Open)) => "Open".to_string(),
        Some(SidebarEntryTarget::Scope(TaskScopeTarget::Workspace)) => "Workspace".to_string(),
        Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(_))) => entry
            .label
            .split_once(' ')
            .map(|(_, name)| name)
            .unwrap_or(&entry.label)
            .trim_end_matches('*')
            .to_string(),
        None => entry.label.clone(),
    }
}

fn sidebar_entry_line(
    entry: &SidebarEntry,
    marker: &str,
    label: &str,
    label_style: Style,
    marker_color: Color,
    active: bool,
    width: usize,
) -> Line<'static> {
    let marker_cell = format!("{marker} ");
    let count = entry.count.to_string();
    let reserved_width = marker_cell.chars().count() + count.chars().count() + 1;
    let label_width = width.saturating_sub(reserved_width);
    let label = truncate_chars(label, label_width);
    let used_width = marker_cell.chars().count() + label.chars().count() + count.chars().count();
    let spacer_width = width.saturating_sub(used_width).max(1);
    let count_style = if active {
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(FG_MUTED).add_modifier(Modifier::BOLD)
    };
    Line::from(vec![
        Span::styled(marker_cell, Style::new().fg(marker_color)),
        Span::styled(label, label_style),
        Span::raw(" ".repeat(spacer_width)),
        Span::styled(count, count_style),
    ])
}

fn filter_item(icon: &str, label: &str, count: i64, color: Color, width: u16) -> ListItem<'static> {
    let icon_cell = if icon == "⚡" {
        format!("{icon} ")
    } else {
        format!("{icon}  ")
    };
    let count_width = if count > 0 { 2 } else { 1 };
    let label_width = (width as usize).saturating_sub(icon_cell.chars().count() + count_width + 2);
    ListItem::new(Line::from(vec![
        Span::styled(icon_cell, Style::new().fg(color)),
        Span::styled(format!("{label:<label_width$}"), Style::new().fg(FG_MUTED)),
        badge(count, false),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_entry_line_truncates_label_and_preserves_count() {
        let entry = SidebarEntry {
            label: "APP very-long-project-name".to_string(),
            count: 12,
            section: false,
            target: Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(
                "very-long-project-name".to_string(),
            ))),
        };

        let rendered = sidebar_entry_line(
            &entry,
            "●",
            "very-long-project-name",
            Style::new().fg(FG),
            FG,
            false,
            14,
        )
        .to_string();

        assert!(rendered.contains("…"));
        assert!(rendered.ends_with("12"));
    }
}
