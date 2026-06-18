use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, List, ListItem, Padding, Paragraph, Row, Table, Wrap,
};

use crate::query::TaskListItem;
use crate::render::quote;
use crate::tui::app::{App, Focus, SidebarTarget};
use crate::tui::theme::{self, ACCENT, BG, BG_ALT, BORDER, FG, FG_DIM, SELECTED};
use crate::tui::widgets::{priority_short, title_cell};

pub(crate) fn render(frame: &mut Frame, app: &mut App) {
    frame.render_widget(Block::new().style(Style::new().bg(BG)), frame.area());

    if frame.area().width < 70 || frame.area().height < 18 {
        frame.render_widget(
            Paragraph::new("terminal too small for atm tui")
                .alignment(Alignment::Center)
                .style(Style::new().fg(FG).bg(BG)),
            frame.area(),
        );
        return;
    }

    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(2),
    ])
    .areas(frame.area());

    let [sidebar, main] =
        Layout::horizontal([Constraint::Length(20), Constraint::Fill(1)]).areas(body);

    frame.render_widget(header_bar(app), header);
    render_sidebar(frame, app, sidebar);
    render_tasks(frame, app, main);
    frame.render_widget(footer_bar(app), footer);

    if app.detail_open
        && let Some(task) = app.selected_task()
    {
        render_detail(frame, task);
    }
    if app.help_open {
        render_help(frame);
    }
    if app.search_open {
        render_search(frame, app);
    }
}

fn header_bar(app: &App) -> Paragraph<'static> {
    let view = match &app.active_view {
        SidebarTarget::All => "All".to_string(),
        SidebarTarget::Inbox => "Inbox".to_string(),
        SidebarTarget::Active => "Active".to_string(),
        SidebarTarget::Project(project) => format!("Project {project}"),
    };
    let search = app
        .filters
        .search
        .as_ref()
        .map(|search| format!("  search: {}", quote(search)))
        .unwrap_or_default();
    Paragraph::new(format!(
        " atm  view: {view}  sort: {}{}                                      ? help",
        app.sort_label(),
        search
    ))
    .style(Style::new().fg(FG).bg(BG))
}

fn footer_bar(app: &App) -> Paragraph<'static> {
    let focus = match app.focus {
        Focus::Sidebar => "sidebar",
        Focus::Tasks => "tasks",
    };
    let message = app
        .message
        .as_deref()
        .map(|message| format!("  {message}"))
        .unwrap_or_default();
    Paragraph::new(format!(
        " focus: {focus}{message}\n j/k move  Tab focus  Enter detail/select  1-6 status  p/P priority  / search  s sort  d delete  u restore  r refresh  q quit"
    ))
    .style(Style::new().fg(FG).bg(BG))
}

fn render_sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    let border_style = if app.focus == Focus::Sidebar {
        Style::new().fg(ACCENT)
    } else {
        Style::new().fg(BORDER)
    };
    let items = app.sidebar_entries.iter().map(|entry| {
        if entry.section {
            return ListItem::new(
                Line::from(entry.label.clone())
                    .style(Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD)),
            );
        }
        let marker = if entry.target.as_ref() == Some(&app.active_view) {
            "> "
        } else {
            "  "
        };
        let label = format!("{marker}{:<10} {:>3}", entry.label, entry.count);
        ListItem::new(Line::from(label).style(Style::new().fg(FG)))
    });
    let list = List::new(items.collect::<Vec<_>>())
        .block(
            Block::new()
                .title("Views")
                .borders(Borders::RIGHT)
                .border_style(border_style)
                .style(Style::new().bg(BG)),
        )
        .highlight_style(SELECTED)
        .highlight_symbol(" ");
    frame.render_stateful_widget(list, area, &mut app.sidebar);
}

fn render_tasks(frame: &mut Frame, app: &mut App, area: Rect) {
    let border_style = if app.focus == Focus::Tasks {
        Style::new().fg(ACCENT)
    } else {
        Style::new().fg(BORDER)
    };
    let header = Row::new(["REF", "P", "STATUS", "PROJECT", "TITLE"]).style(
        Style::new()
            .fg(ACCENT)
            .bg(BG_ALT)
            .add_modifier(Modifier::BOLD),
    );

    let rows = app.tasks.iter().map(|item| {
        Row::new([
            Cell::from(item.display_ref.clone()),
            Cell::from(priority_short(&item.task.priority))
                .style(theme::priority_style(&item.task.priority)),
            Cell::from(item.task.status.clone()).style(theme::status_style(&item.task.status)),
            Cell::from(item.task.project_key.clone()),
            Cell::from(title_cell(item)),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(2),
            Constraint::Length(10),
            Constraint::Length(14),
            Constraint::Fill(1),
        ],
    )
    .header(header)
    .block(
        Block::new()
            .title("Tasks")
            .borders(Borders::LEFT)
            .border_style(border_style)
            .style(Style::new().bg(BG)),
    )
    .row_highlight_style(SELECTED)
    .highlight_symbol(" ");

    frame.render_stateful_widget(table, area, &mut app.table);
}

fn render_detail(frame: &mut Frame, item: &TaskListItem) {
    let area = centered(frame.area(), 72, 12);
    frame.render_widget(Clear, area);
    let labels = if item.labels.is_empty() {
        "none".to_string()
    } else {
        item.labels.join(",")
    };
    let deleted = if item.task.deleted { " yes" } else { " no" };
    let text = Text::from(vec![
        Line::from(vec![
            Span::styled(
                &item.display_ref,
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                &item.task.title,
                Style::new().fg(FG).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(format!(
            "project={} status={} priority={} deleted={}",
            item.task.project_key, item.task.status, item.task.priority, deleted
        )),
        Line::from(format!(
            "created={} updated={}",
            item.task.created_at, item.task.updated_at
        )),
        Line::from(format!("labels={labels}")),
        Line::from(""),
        Line::from(if item.task.description.is_empty() {
            "(no description)".to_string()
        } else {
            item.task.description.clone()
        }),
    ]);
    let block = overlay_block("Detail");
    frame.render_widget(
        Paragraph::new(text)
            .block(block)
            .wrap(Wrap { trim: false })
            .style(Style::new().fg(FG).bg(BG_ALT)),
        area,
    );
}

fn render_help(frame: &mut Frame) {
    let area = centered(frame.area(), 64, 11);
    frame.render_widget(Clear, area);
    let text = Text::from(vec![
        Line::from("j/k or arrows move the focused list"),
        Line::from("Tab switches between views and tasks"),
        Line::from("Enter selects a view or toggles task detail"),
        Line::from("1 inbox  2 backlog  3 todo  4 active  5 done  6 canceled"),
        Line::from("p/P cycle priority, d deletes, u restores"),
        Line::from("/ searches title and description, s cycles sort"),
        Line::from("r refreshes from SQLite, q quits"),
    ]);
    frame.render_widget(
        Paragraph::new(text)
            .block(overlay_block("Help"))
            .style(Style::new().fg(FG).bg(BG_ALT)),
        area,
    );
}

fn render_search(frame: &mut Frame, app: &App) {
    let area = centered(frame.area(), 54, 3);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(format!("/{}", app.search_input))
            .block(overlay_block("Search"))
            .style(Style::new().fg(FG).bg(BG_ALT)),
        area,
    );
}

fn overlay_block(title: &'static str) -> Block<'static> {
    Block::new()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .padding(Padding::horizontal(1))
        .style(Style::new().bg(BG_ALT))
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [area] = Layout::horizontal([Constraint::Length(width.min(area.width.saturating_sub(2)))])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::vertical([Constraint::Length(
        height.min(area.height.saturating_sub(2)),
    )])
    .flex(Flex::Center)
    .areas(area);
    area
}
