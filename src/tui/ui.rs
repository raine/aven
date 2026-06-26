mod detail;
mod dialog;
mod footer;
mod header;

pub(crate) use self::header::{HeaderTarget, header_target_at};
mod input;
mod overlays;
mod shortcuts;
mod sidebar;
mod task_display;
mod task_list;
mod toast;
mod truncate;

pub(crate) use self::sidebar::{sidebar_click_at, sidebar_layout};

use self::detail::render_detail_underlay;
use self::footer::{FooterMode, footer_bar};
use self::header::render_header;
use self::overlays::{
    render_confirm, render_database_stats, render_multiline_input, render_picker, render_search,
    render_sync_status, render_tag_combobox, render_text_input, render_text_panel,
};
use self::shortcuts::{render_command, render_detail_help, render_help, render_prefix_hints};
use self::sidebar::{render_sidebar, render_sidebar_overlay};
use self::task_list::render_tasks;
use self::toast::render_toast;

pub(crate) use self::detail::{DetailMetadataTarget, detail_metadata_target_at, detail_scroll_cap};
pub(crate) use self::overlays::{database_stats_scroll_cap, text_panel_scroll_cap};
pub(crate) use self::shortcuts::{detail_help_scroll_cap, help_scroll_cap};
pub(crate) use self::task_list::{task_at_position, task_status_at_position};

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph};

use crate::tui::app::{Focus, WidgetState};
use crate::tui::overlay::{
    HeaderMenuKind, HeaderMenuView, OrderMenuView, OverlayRoute, OverlayView, TextInputView,
};
use crate::tui::store::{TaskOrder, TuiStore};
use crate::tui::theme::{ACCENT, BG, BG_ALT, BG_PANEL, FG, FG_DIM, GREEN, PINK, SELECTED};
use crate::tui::toast::Toast;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewSurface {
    Main,
    AddTask,
}

#[derive(Clone)]
pub(crate) struct ViewState {
    pub(crate) focus: Focus,
    pub(crate) overlay: Option<OverlayView>,
    pub(crate) detail_underlay: bool,
    pub(crate) notification: Option<Toast>,
    pub(crate) pending_shortcut: Vec<String>,
    pub(crate) surface: ViewSurface,
}

impl ViewState {
    fn footer_mode(&self) -> FooterMode {
        if matches!(
            self.overlay,
            Some(OverlayView::Detail { .. } | OverlayView::DetailHelp { .. })
        ) {
            FooterMode::Detail
        } else {
            FooterMode::List
        }
    }
}

fn detail_underlay_scroll(overlay: &Option<OverlayView>) -> u16 {
    match overlay {
        Some(OverlayView::Detail { scroll }) => *scroll,
        Some(OverlayView::DetailHelp { .. }) => 0,
        _ => 0,
    }
}

pub(crate) fn render(
    frame: &mut Frame,
    store: &TuiStore,
    widgets: &mut WidgetState,
    view: &ViewState,
) {
    frame.render_widget(Block::new().style(Style::new().bg(BG)), frame.area());

    if view.surface == ViewSurface::AddTask {
        render_add_task_surface(frame, view);
        return;
    }

    if frame.area().width < 70 || frame.area().height < 18 {
        frame.render_widget(
            Paragraph::new("terminal too small for aven tui")
                .alignment(Alignment::Center)
                .style(Style::new().fg(FG).bg(BG)),
            frame.area(),
        );
        return;
    }

    let inner = frame.area();

    let [header, body, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(2),
    ])
    .areas(inner);

    render_header(frame, store, header);
    let inline_title_editor = inline_title_editor(view);
    let inline_detail_title_editor = inline_detail_title_editor(view);
    if body.width < 100 {
        render_tasks(frame, store, widgets, view.focus, body, inline_title_editor);
        if let Some(layout) = crate::tui::ui::sidebar_layout(inner, view.focus)
            && layout.overlay
        {
            render_sidebar_overlay(frame, store, widgets, view.focus, body);
        }
    } else {
        let layout = crate::tui::ui::sidebar_layout(inner, view.focus).unwrap();
        let main = ratatui::layout::Rect {
            x: layout.sidebar.x.saturating_add(layout.sidebar.width),
            y: body.y,
            width: body.width.saturating_sub(layout.sidebar.width),
            height: body.height,
        };
        render_sidebar(
            frame,
            store,
            widgets,
            view.focus,
            layout.sidebar,
            layout.overlay,
        );
        render_tasks(frame, store, widgets, view.focus, main, inline_title_editor);
    }
    frame.render_widget(footer_bar(view.footer_mode(), footer.width), footer);

    if view.detail_underlay {
        render_detail_underlay(
            frame,
            store,
            widgets,
            detail_underlay_scroll(&view.overlay),
            inline_detail_title_editor,
        );
    }
    if let Some(overlay) = &view.overlay {
        render_overlay(
            frame,
            store,
            widgets,
            overlay,
            inline_title_editor.is_some() || inline_detail_title_editor.is_some(),
        );
    }
    if !view.pending_shortcut.is_empty() && !add_task_dialog_prefix_active(view) {
        render_prefix_hints(frame, view);
    }
    if let Some(toast) = &view.notification {
        render_toast(frame, toast);
    }
}

fn add_task_dialog_prefix_active(view: &ViewState) -> bool {
    matches!(
        &view.overlay,
        Some(OverlayView::AddTask(state))
            if state.status_prefix_active || state.priority_prefix_active
    )
}

fn render_add_task_surface(frame: &mut Frame, view: &ViewState) {
    if frame.area().width < 30 || frame.area().height < 8 {
        frame.render_widget(
            Paragraph::new("terminal too small for add task")
                .alignment(Alignment::Center)
                .style(Style::new().fg(FG).bg(BG)),
            frame.area(),
        );
        return;
    }

    if let Some(overlay) = &view.overlay {
        render_add_task_surface_overlay(frame, view, overlay);
    }
    if !view.pending_shortcut.is_empty() && !add_task_dialog_prefix_active(view) {
        render_prefix_hints(frame, view);
    }
    if let Some(toast) = &view.notification {
        render_toast(frame, toast);
    }
}

fn render_add_task_surface_overlay(frame: &mut Frame, _view: &ViewState, overlay: &OverlayView) {
    match overlay {
        OverlayView::AddTask(state) => self::overlays::render_add_task_full_frame(frame, state),
        OverlayView::MultilineInput(state)
            if matches!(
                state.route,
                OverlayRoute::AddTaskDescription | OverlayRoute::AddTaskNatural
            ) =>
        {
            render_add_task_multiline_full_frame(frame, state)
        }
        _ => render_overlay_content(frame, overlay, false),
    }
}

fn render_add_task_multiline_full_frame(
    frame: &mut Frame,
    state: &crate::tui::overlay::MultilineInputView,
) {
    use crate::tui::overlay::OverlayRoute::{AddTaskDescription, AddTaskNatural};

    let placeholder = match state.route {
        AddTaskDescription => "Optional details, links, or handoff context...",
        AddTaskNatural => "Describe the task in natural language...",
        _ => return,
    };
    let hint_line = match state.route {
        AddTaskDescription => self::overlays::add_task_description_hint_line(),
        AddTaskNatural => self::overlays::add_task_natural_hint_line(),
        _ => return,
    };
    let content = dialog::Dialog::new(&state.title, frame.area().width, frame.area().height)
        .render_block_at(frame, frame.area());
    let visible_rows = content.height.saturating_sub(2).max(1) as usize;
    let start = self::overlays::tail_viewport_start(state.row, visible_rows);
    let mut lines = Vec::new();
    for (row_index, line) in state
        .lines
        .iter()
        .enumerate()
        .skip(start)
        .take(visible_rows)
    {
        lines.push(self::overlays::add_task_free_text_input_line(
            line,
            if row_index == state.row {
                Some(state.column)
            } else {
                None
            },
            line.is_empty() && state.lines.len() == 1,
            placeholder,
        ));
    }
    while lines.len() + 1 < content.height as usize {
        lines.push(Line::from(""));
    }
    lines.push(hint_line);
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::new().fg(FG).bg(crate::tui::theme::BG_ALT)),
        content,
    );
}

fn edit_title_view(view: &ViewState) -> Option<&TextInputView> {
    match &view.overlay {
        Some(OverlayView::TextInput(state)) if state.route == OverlayRoute::EditTitle => {
            Some(state)
        }
        _ => None,
    }
}

fn inline_title_editor(view: &ViewState) -> Option<&TextInputView> {
    if view.focus != Focus::Tasks || view.detail_underlay {
        return None;
    }
    edit_title_view(view)
}

fn inline_detail_title_editor(view: &ViewState) -> Option<&TextInputView> {
    if !view.detail_underlay {
        return None;
    }
    edit_title_view(view)
}

fn render_header_menu(frame: &mut Frame, state: &HeaderMenuView) {
    use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

    let menu_state = crate::tui::overlay::HeaderMenuState {
        kind: state.kind,
        column: state.column,
        row: state.row,
        selected: state.selected,
        items: state.items.clone(),
    };
    let area = menu_state.area(frame.area().width, frame.area().height);
    frame.render_widget(Clear, area);
    let block = Block::new()
        .title(menu_title(header_menu_title(state.kind)))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .style(Style::new().bg(BG_ALT));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let prefix_width = if matches!(state.kind, HeaderMenuKind::Scope) {
        state
            .items
            .iter()
            .map(|item| project_prefix_and_name(&item.label).map_or(0, |(prefix, _)| prefix.len()))
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    let lines = state
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            header_menu_line(
                state.kind,
                index == state.selected,
                &item.key,
                &item.label,
                prefix_width,
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::new().fg(FG).bg(BG_ALT)),
        inner,
    );
}

fn render_order_menu(frame: &mut Frame, state: &OrderMenuView) {
    use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

    let menu_state = crate::tui::overlay::OrderMenuState {
        column: state.column,
        row: state.row,
        selected: state.selected,
    };
    let area = menu_state.area(frame.area().width, frame.area().height);
    frame.render_widget(Clear, area);
    let block = Block::new()
        .title(menu_title("order"))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .style(Style::new().bg(BG_ALT));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines = Vec::new();
    for (order, key, label) in order_menu_items() {
        lines.push(order_menu_line(order, key, label, state.selected));
    }
    lines.push(Line::from(vec![
        Span::styled("Esc", Style::new().fg(FG).add_modifier(Modifier::BOLD)),
        Span::styled(" close", Style::new().fg(FG_DIM)),
    ]));
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::new().fg(FG).bg(BG_ALT)),
        inner,
    );
}

fn menu_title(title: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled("─ ", Style::new().fg(ACCENT)),
        Span::styled(title, Style::new().fg(FG).add_modifier(Modifier::BOLD)),
        Span::styled(" ", Style::new().fg(ACCENT)),
    ])
}

fn header_menu_title(kind: HeaderMenuKind) -> &'static str {
    match kind {
        HeaderMenuKind::Workspace => "workspace",
        HeaderMenuKind::Scope => "scope",
        HeaderMenuKind::View => "view",
        HeaderMenuKind::Status => "status",
        HeaderMenuKind::Priority => "priority",
    }
}

fn header_menu_line(
    kind: HeaderMenuKind,
    selected: bool,
    key: &str,
    label: &str,
    prefix_width: usize,
) -> Line<'static> {
    let row_style = if selected {
        SELECTED
    } else {
        Style::new().fg(FG).bg(BG_PANEL)
    };
    let marker = if selected { "▸" } else { " " };
    let mut spans = vec![
        Span::styled(format!("{marker} "), row_style),
        Span::styled(format!("{key:<2}"), row_style.add_modifier(Modifier::BOLD)),
        Span::styled(" ", row_style),
    ];
    if matches!(kind, HeaderMenuKind::Scope)
        && let Some((prefix, name)) = project_prefix_and_name(label)
    {
        spans.extend([
            Span::styled(
                format!("{prefix:<prefix_width$}"),
                row_style
                    .fg(crate::tui::theme::project_color(name))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", row_style),
            Span::styled(name.to_string(), row_style),
        ]);
    } else {
        spans.push(Span::styled(
            label.to_string(),
            header_menu_label_style(kind, label, row_style, selected),
        ));
    }
    Line::from(spans)
}

fn header_menu_label_style(
    kind: HeaderMenuKind,
    label: &str,
    row_style: Style,
    selected: bool,
) -> Style {
    if matches!(kind, HeaderMenuKind::View | HeaderMenuKind::Status) {
        let bg = row_style.bg.unwrap_or(BG_PANEL);
        let style = match label {
            "queue" => Style::new().fg(ACCENT).bg(bg),
            "open" => Style::new().fg(GREEN).bg(bg),
            "conflicts" => Style::new().fg(PINK).bg(bg),
            "active" | "todo" | "inbox" | "backlog" | "done" | "canceled" => {
                crate::tui::theme::status_style(label).bg(bg)
            }
            _ => row_style,
        };
        if selected {
            style.add_modifier(Modifier::BOLD)
        } else {
            style
        }
    } else if matches!(kind, HeaderMenuKind::Priority) {
        let bg = row_style.bg.unwrap_or(BG_PANEL);
        let style = crate::tui::theme::priority_style(label).bg(bg);
        if selected {
            style.add_modifier(Modifier::BOLD)
        } else {
            style
        }
    } else {
        row_style
    }
}

fn project_prefix_and_name(label: &str) -> Option<(&str, &str)> {
    label
        .split_once(' ')
        .filter(|(prefix, name)| !prefix.is_empty() && !name.is_empty())
}

fn order_menu_line(
    order: TaskOrder,
    key: &'static str,
    label: &'static str,
    selected: TaskOrder,
) -> Line<'static> {
    let row_style = if order == selected {
        SELECTED
    } else {
        Style::new().fg(FG).bg(BG_PANEL)
    };
    let marker = if order == selected { "▸" } else { " " };
    Line::from(vec![
        Span::styled(format!("{marker} "), row_style),
        Span::styled(format!("{key:<2}"), row_style.add_modifier(Modifier::BOLD)),
        Span::styled(" ", row_style),
        Span::styled(label, row_style),
    ])
}

fn order_menu_items() -> [(TaskOrder, &'static str, &'static str); 5] {
    [
        (TaskOrder::Created, "c", "created"),
        (TaskOrder::Updated, "u", "updated"),
        (TaskOrder::Priority, "p", "priority"),
        (TaskOrder::Project, "g", "project"),
        (TaskOrder::Title, "t", "title"),
    ]
}

fn render_overlay_content(frame: &mut Frame, overlay: &OverlayView, inline_title_editor: bool) {
    match overlay {
        OverlayView::Help { scroll } => render_help(frame, *scroll),
        OverlayView::DetailHelp { scroll } => render_detail_help(frame, *scroll),
        OverlayView::Search { input, cursor } => render_search(frame, input, *cursor),
        OverlayView::Command {
            input,
            cursor,
            cycle_input,
            highlighted,
        } => render_command(
            frame,
            input,
            *cursor,
            cycle_input.as_deref(),
            highlighted.as_deref(),
        ),
        OverlayView::AddTask(state) => self::overlays::render_add_task(frame, state),
        OverlayView::TextInput(state)
            if state.route == OverlayRoute::EditTitle && inline_title_editor => {}
        OverlayView::TextInput(state) => render_text_input(frame, state),
        OverlayView::MultilineInput(state) => render_multiline_input(frame, state),
        OverlayView::Picker(state) => render_picker(frame, state),
        OverlayView::TagCombobox(state) => render_tag_combobox(frame, state),
        OverlayView::HeaderMenu(state) => render_header_menu(frame, state),
        OverlayView::OrderMenu(state) => render_order_menu(frame, state),
        OverlayView::Confirm(state) => render_confirm(frame, state),
        OverlayView::TextPanel(state) => render_text_panel(frame, state),
        OverlayView::SyncStatus(state) => render_sync_status(frame, state),
        OverlayView::DatabaseStats { stats, scroll } => {
            render_database_stats(frame, stats, *scroll)
        }
        OverlayView::Detail { .. } => {}
    }
}

fn render_overlay(
    frame: &mut Frame,
    store: &TuiStore,
    widgets: &mut WidgetState,
    overlay: &OverlayView,
    inline_title_editor: bool,
) {
    if matches!(
        overlay,
        OverlayView::Detail { .. } | OverlayView::DetailHelp { .. }
    ) {
        let scroll = match overlay {
            OverlayView::Detail { scroll } => *scroll,
            OverlayView::DetailHelp { .. } => 0,
            _ => 0,
        };
        render_detail_underlay(frame, store, widgets, scroll, None);
        if matches!(overlay, OverlayView::DetailHelp { .. }) {
            render_overlay_content(frame, overlay, inline_title_editor);
        }
        return;
    }
    render_overlay_content(frame, overlay, inline_title_editor);
}
