mod columns;
mod detail;
mod dialog;
mod empty_state;
mod footer;
mod header;

pub(crate) use self::header::{HeaderTarget, header_target_at};
mod input;
mod overlays;
mod recent_actions;
mod recurrence;
mod scroll;
mod shortcuts;
mod sidebar;
mod splash;
mod task_display;
mod task_list;
mod timestamps;
mod toast;
mod truncate;

#[cfg(test)]
pub(crate) use self::sidebar::sidebar_layout;
pub(crate) use self::sidebar::{sidebar_click_at_for, sidebar_layout_for};

pub(crate) use self::columns::column_lane_at_position;
use self::columns::render_columns;
use self::detail::{render_attachment_preview, render_detail_underlay};
pub(crate) use self::footer::bulk_footer_action_at;
use self::footer::{FooterMode, footer_bar};
use self::header::render_header;
pub(crate) use self::overlays::recurrence_history_entry_at;
use self::overlays::{
    SearchRenderStatus, SearchRenderView, render_confirm, render_database_stats,
    render_multiline_input, render_onboarding, render_onboarding_raised, render_picker,
    render_recurrence_history, render_search, render_sync_status, render_tag_combobox,
    render_text_input, render_text_panel, render_update,
};
use self::recent_actions::render_recent_actions;
use self::recurrence::{render_recurrence_detail, render_recurrence_series};
use self::shortcuts::{
    CommandRenderContext, render_command, render_detail_help, render_help, render_prefix_hints,
};
use self::sidebar::{render_sidebar, render_sidebar_overlay};
use self::task_list::render_tasks;
pub(crate) use self::task_list::{
    task_index_at_visual_row, task_visual_row, task_visual_row_count,
};
use self::toast::render_toast;

pub(crate) use self::detail::{
    DetailDocument, DetailInlineImageContext, DetailInlineImagePlacement, DetailMetadataTarget,
    DetailRenderContext, attachment_is_locally_openable, attachment_is_locally_previewable,
    detail_copy_target_at, detail_metadata_target_at, detail_scroll_cap_with_images,
    detail_selected_text, detail_target_is_actionable,
};
#[cfg(test)]
pub(crate) use self::detail::{
    detail_attachment_at_position, detail_attachment_scroll_target, detail_child_task_at_position,
    detail_scroll_cap, detail_section_scroll_target,
};
pub(crate) use self::overlays::{
    AddTaskLayout, add_task_field_at, changelog_link_at, composer_help_scroll_cap,
    database_stats_scroll_cap, text_panel_scroll_cap, update_action_at, update_dialog_size,
    update_link_at, update_notes_scroll_cap,
};
pub(crate) use self::recent_actions::recent_action_at_position;
pub(crate) use self::recurrence::recurrence_series_at_position;
pub(crate) use self::shortcuts::{detail_help_scroll_cap, help_scroll_cap, prefix_hint_scroll_cap};
pub(crate) use self::splash::{render_dimmed_onboarding_splash, render_onboarding_splash};
pub(crate) use self::task_list::{task_at_position, task_status_at_position};

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::tui::app::{Focus, FooterChoiceMode, WidgetState};
use crate::tui::list_surface::ListSurface;
use crate::tui::overlay::{
    HeaderMenuKind, HeaderMenuView, MultilineInputKind, OrderMenuView, OverlayView, TextInputKind,
    TextInputView,
};
use crate::tui::store::{TaskOrder, TaskView, TuiStore};
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
    pub(crate) onboarding_intro: Option<crate::tui::app_onboarding::OnboardingIntroVisual>,
    pub(crate) detail_underlay: bool,
    pub(crate) detail_underlay_scroll: u16,
    pub(crate) detail_has_parent: bool,
    pub(crate) detail_focus: Option<crate::tui::app::DetailTargetId>,
    pub(crate) detail_hover: Option<crate::tui::app::DetailTargetId>,
    pub(crate) detail_expanded_sections: std::collections::BTreeSet<crate::tui::app::DetailSection>,
    pub(crate) removed_epic_child: Option<crate::tui::app::RemovedEpicChild>,
    pub(crate) detail_text_selection: Option<crate::tui::detail_selection::DetailTextSelection>,
    pub(crate) notification: Option<Toast>,
    pub(crate) pending_shortcut: Vec<String>,
    pub(crate) pending_shortcut_scroll: u16,
    pub(crate) copy_description_available: bool,
    pub(crate) copy_notes_available: bool,
    pub(crate) visible_marked_task_count: usize,
    pub(crate) footer_choice_mode: Option<FooterChoiceMode>,
    pub(crate) sidebar_visible: bool,
    pub(crate) update_badge: Option<crate::tui::app_update::UpdateBadgeView>,
    pub(crate) surface: ViewSurface,
    pub(crate) inline_images: Option<DetailInlineImageContext>,
    pub(crate) pending_attachments: Vec<crate::tui::attachment_controller::PendingAttachmentView>,
}

impl ViewState {
    fn footer_mode(&self, width: u16) -> FooterMode {
        match self.footer_choice_mode {
            Some(FooterChoiceMode::Status) => return FooterMode::StatusChoice,
            Some(FooterChoiceMode::Priority) => return FooterMode::PriorityChoice,
            None => {}
        }
        if matches!(self.overlay, Some(OverlayView::AttachmentPreview { .. })) {
            FooterMode::AttachmentPreview
        } else if self.detail_underlay
            || matches!(self.overlay, Some(OverlayView::DetailHelp { .. }))
        {
            if matches!(
                self.detail_focus,
                Some(crate::tui::app::DetailTargetId::Note { .. })
            ) {
                FooterMode::DetailNote
            } else if matches!(
                self.detail_focus,
                Some(crate::tui::app::DetailTargetId::Attachment { .. })
            ) {
                FooterMode::DetailAttachment
            } else if matches!(
                self.detail_focus,
                Some(crate::tui::app::DetailTargetId::Task {
                    section: crate::tui::app::DetailSection::EpicChildren,
                    ..
                })
            ) {
                FooterMode::DetailEpicChild
            } else if self.detail_focus.is_some() {
                FooterMode::DetailLinks
            } else if self
                .detail_text_selection
                .as_ref()
                .is_some_and(|selection| selection.terminal_width == width)
            {
                FooterMode::DetailSelection
            } else if self.detail_has_parent {
                FooterMode::DetailNested
            } else {
                FooterMode::Detail
            }
        } else {
            FooterMode::List
        }
    }
}

fn detail_underlay_scroll(view: &ViewState) -> u16 {
    if matches!(view.overlay, Some(OverlayView::DetailHelp { .. })) {
        0
    } else {
        view.detail_underlay_scroll
    }
}

pub(crate) const MIN_TUI_WIDTH: u16 = 70;
pub(crate) const MIN_TUI_HEIGHT: u16 = 18;

pub(crate) fn footer_area(terminal: Rect) -> Rect {
    Rect {
        x: terminal.x,
        y: terminal.y.saturating_add(terminal.height.saturating_sub(2)),
        width: terminal.width,
        height: terminal.height.min(2),
    }
}

pub(crate) fn render(
    frame: &mut Frame,
    store: &TuiStore,
    widgets: &mut WidgetState,
    list: &mut ListSurface,
    view: &ViewState,
) {
    render_surface(frame, store, widgets, list, view);
    widgets.text_cursor = self::input::text_cursor_position(frame.buffer_mut());
}

fn render_surface(
    frame: &mut Frame,
    store: &TuiStore,
    widgets: &mut WidgetState,
    list: &mut ListSurface,
    view: &ViewState,
) {
    widgets.inline_image_placements.clear();
    frame.render_widget(Block::new().style(Style::new().bg(BG)), frame.area());

    if view.surface == ViewSurface::AddTask {
        render_add_task_surface(frame, view);
        return;
    }

    if frame.area().width < MIN_TUI_WIDTH || frame.area().height < MIN_TUI_HEIGHT {
        frame.render_widget(
            Paragraph::new("terminal too small for aven tui")
                .alignment(Alignment::Center)
                .style(Style::new().fg(FG).bg(BG)),
            frame.area(),
        );
        return;
    }

    if matches!(
        view.overlay,
        Some(OverlayView::Onboarding {
            splash_underlay: true
        })
    ) {
        if let Some(intro) = view.onboarding_intro {
            render_onboarding_splash(frame, intro);
            render_onboarding_raised(frame, intro.dialog_reveal);
        } else {
            render_dimmed_onboarding_splash(frame);
            render_onboarding(frame);
        }
        return;
    }

    let inner = frame.area();

    let [header, body, _] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(2),
    ])
    .areas(inner);
    let footer = footer_area(inner);

    render_header(frame, store, view.update_badge.as_ref(), header);
    let inline_title_editor = inline_title_editor(view);
    let inline_detail_title_editor = inline_detail_title_editor(view);
    if body.width < 100 {
        render_main_surface(
            frame,
            store,
            widgets,
            list,
            view.focus,
            body,
            inline_title_editor,
        );
        if let Some(layout) =
            crate::tui::ui::sidebar_layout_for(inner, view.focus, view.sidebar_visible)
            && layout.overlay
        {
            render_sidebar_overlay(frame, store, list, view.focus, body);
        }
    } else {
        if let Some(layout) =
            crate::tui::ui::sidebar_layout_for(inner, view.focus, view.sidebar_visible)
        {
            let main = ratatui::layout::Rect {
                x: layout.sidebar.x.saturating_add(layout.sidebar.width),
                y: body.y,
                width: body.width.saturating_sub(layout.sidebar.width),
                height: body.height,
            };
            render_sidebar(
                frame,
                store,
                list,
                view.focus,
                layout.sidebar,
                layout.overlay,
            );
            render_main_surface(
                frame,
                store,
                widgets,
                list,
                view.focus,
                main,
                inline_title_editor,
            );
        } else {
            render_main_surface(
                frame,
                store,
                widgets,
                list,
                view.focus,
                body,
                inline_title_editor,
            );
        }
    }
    let footer_mode = match view.footer_mode(footer.width) {
        FooterMode::List if store.view_state.view == TaskView::Columns => FooterMode::Columns,
        FooterMode::Detail | FooterMode::DetailNested
            if store.view_state.view == TaskView::Recurring =>
        {
            match store
                .recurrence_detail
                .as_ref()
                .map(|detail| detail.series.state)
            {
                Some(aven_core::recurrence::RecurrenceSeriesState::Active) => {
                    FooterMode::RecurrenceDetailActive
                }
                Some(aven_core::recurrence::RecurrenceSeriesState::Paused) => {
                    FooterMode::RecurrenceDetailPaused
                }
                Some(aven_core::recurrence::RecurrenceSeriesState::Stopped) => {
                    FooterMode::RecurrenceDetailStopped
                }
                None => FooterMode::Detail,
            }
        }
        mode @ (FooterMode::Detail | FooterMode::DetailNested)
            if store
                .selected_task(list.selected_task())
                .is_some_and(|item| item.task.deleted) =>
        {
            if mode == FooterMode::DetailNested {
                FooterMode::DetailNestedDeleted
            } else {
                FooterMode::DetailDeleted
            }
        }
        mode => mode,
    };
    frame.render_widget(
        footer_bar(footer_mode, footer.width, view.visible_marked_task_count),
        footer,
    );

    if view.detail_underlay {
        if store.view_state.view == TaskView::Recurring {
            if let Some(detail) = store.recurrence_detail.as_ref() {
                render_recurrence_detail(frame, detail, detail_underlay_scroll(view));
            }
        } else {
            render_detail_underlay(
                frame,
                store,
                widgets,
                list.selected_task(),
                detail_underlay_scroll(view),
                inline_detail_title_editor,
                view.detail_focus.as_ref(),
                view.detail_hover.as_ref(),
                &view.detail_expanded_sections,
                view.detail_text_selection.as_ref(),
                view.inline_images.as_ref(),
                &view.pending_attachments,
                view.removed_epic_child.as_ref(),
            );
        }
    }
    if let Some(overlay) = &view.overlay {
        render_overlay(
            frame,
            store,
            widgets,
            list,
            overlay,
            inline_title_editor.is_some() || inline_detail_title_editor.is_some(),
            view.detail_focus.as_ref(),
            view.detail_hover.as_ref(),
            &view.detail_expanded_sections,
            view.detail_text_selection.as_ref(),
            view.inline_images.as_ref(),
            &view.pending_attachments,
            view.removed_epic_child.as_ref(),
        );
    }
    if !view.pending_shortcut.is_empty() && !add_task_dialog_prefix_active(view) {
        render_prefix_hints(frame, view);
    }
    if let Some(toast) = &view.notification {
        render_toast(frame, toast);
    }
}

fn render_main_surface(
    frame: &mut Frame,
    store: &TuiStore,
    _widgets: &mut WidgetState,
    list: &mut ListSurface,
    focus: Focus,
    area: ratatui::layout::Rect,
    inline_title_editor: Option<&TextInputView>,
) {
    if store.view_state.view == TaskView::RecentActions {
        render_recent_actions(frame, store, list, focus, area);
    } else if store.view_state.view == TaskView::Recurring {
        render_recurrence_series(frame, store, list, focus, area);
    } else if store.view_state.view == TaskView::Columns {
        let marked_task_ids = list.marked_task_ids().clone();
        render_columns(
            frame,
            store,
            list.table_state_mut(),
            focus,
            area,
            inline_title_editor,
            &marked_task_ids,
        );
    } else {
        render_tasks(frame, store, list, focus, area, inline_title_editor);
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
                state.kind,
                MultilineInputKind::AddTaskDescription | MultilineInputKind::AddTaskNatural
            ) =>
        {
            render_add_task_multiline_full_frame(frame, state)
        }
        _ => {
            if overlay_dims_underlay(overlay, false) {
                dialog::dim_rendered_background(frame);
            }
            render_overlay_content(frame, overlay, false);
        }
    }
}

fn render_add_task_multiline_full_frame(
    frame: &mut Frame,
    state: &crate::tui::overlay::MultilineInputView,
) {
    let placeholder = match state.kind {
        MultilineInputKind::AddTaskDescription => "Optional details, links, or handoff context...",
        MultilineInputKind::AddTaskNatural => "Describe the task in natural language...",
        _ => return,
    };
    let hint_line = match state.kind {
        MultilineInputKind::AddTaskDescription => self::overlays::add_task_description_hint_line(),
        MultilineInputKind::AddTaskNatural => self::overlays::add_task_natural_hint_line(),
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
        Some(OverlayView::TextInput(state)) if state.kind == TextInputKind::EditTitle => {
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
            .map(|item| {
                project_prefix_and_name(&item.label).map_or(0, |(prefix, _)| prefix.width())
            })
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
        let padding = prefix_width.saturating_sub(prefix.width());
        spans.extend([
            Span::styled(
                format!("{prefix}{}", " ".repeat(padding)),
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
    if matches!(kind, HeaderMenuKind::View) {
        let bg = row_style.bg.unwrap_or(BG_PANEL);
        let style = match label {
            "queue" => Style::new().fg(ACCENT).bg(bg),
            "open" => Style::new().fg(GREEN).bg(bg),
            "conflicts" => Style::new().fg(PINK).bg(bg),
            _ => row_style,
        };
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

fn order_menu_items() -> [(TaskOrder, &'static str, &'static str); 6] {
    [
        (TaskOrder::DueOn, "d", "due"),
        (TaskOrder::Created, "c", "created"),
        (TaskOrder::Updated, "u", "updated"),
        (TaskOrder::Priority, "p", "priority"),
        (TaskOrder::Project, "g", "project"),
        (TaskOrder::Title, "t", "title"),
    ]
}

fn overlay_dims_underlay(overlay: &OverlayView, inline_title_editor: bool) -> bool {
    match overlay {
        OverlayView::Onboarding { .. }
        | OverlayView::Help { .. }
        | OverlayView::DetailHelp { .. }
        | OverlayView::Search { .. }
        | OverlayView::Command { .. }
        | OverlayView::AddTask(_)
        | OverlayView::MultilineInput(_)
        | OverlayView::Picker(_)
        | OverlayView::TagCombobox(_)
        | OverlayView::Confirm(_)
        | OverlayView::TextPanel(_)
        | OverlayView::Changelog { .. }
        | OverlayView::RecurrenceHistory(_)
        | OverlayView::SyncStatus(_)
        | OverlayView::DatabaseStats { .. }
        | OverlayView::Update(_) => true,
        OverlayView::TextInput(state) => {
            !(inline_title_editor && state.kind == TextInputKind::EditTitle)
        }
        OverlayView::Detail { .. }
        | OverlayView::AttachmentPreview { .. }
        | OverlayView::HeaderMenu(_)
        | OverlayView::OrderMenu(_) => false,
    }
}

fn render_overlay_content(frame: &mut Frame, overlay: &OverlayView, inline_title_editor: bool) {
    match overlay {
        OverlayView::Onboarding { .. } => render_onboarding(frame),
        OverlayView::Help { scroll } => render_help(frame, *scroll),
        OverlayView::DetailHelp { scroll } => render_detail_help(frame, *scroll, None),
        OverlayView::Search {
            input,
            cursor,
            results,
            selected,
            total_matches,
            stale,
            no_matches_cached,
            intent,
        } => render_search(
            frame,
            SearchRenderView {
                input,
                cursor: *cursor,
                results,
                selected: *selected,
                total_matches: *total_matches,
                status: SearchRenderStatus {
                    stale: *stale,
                    no_matches_cached: *no_matches_cached,
                },
                intent,
            },
        ),
        OverlayView::Command {
            input,
            cursor,
            cycle_input,
            highlighted,
            context,
            marked_task_count,
            unavailable,
        } => render_command(
            frame,
            input,
            *cursor,
            cycle_input.as_deref(),
            highlighted.as_deref(),
            CommandRenderContext {
                unavailable,
                command_context: *context,
                marked_task_count: *marked_task_count,
            },
        ),
        OverlayView::AddTask(state) => self::overlays::render_add_task(frame, state),
        OverlayView::TextInput(state)
            if state.kind == TextInputKind::EditTitle && inline_title_editor => {}
        OverlayView::TextInput(state) => render_text_input(frame, state),
        OverlayView::MultilineInput(state) => render_multiline_input(frame, state),
        OverlayView::Picker(state) => render_picker(frame, state),
        OverlayView::TagCombobox(state) => render_tag_combobox(frame, state),
        OverlayView::HeaderMenu(state) => render_header_menu(frame, state),
        OverlayView::OrderMenu(state) => render_order_menu(frame, state),
        OverlayView::Confirm(state) => render_confirm(frame, state),
        OverlayView::TextPanel(state) => render_text_panel(frame, state),
        OverlayView::Changelog { markdown, scroll } => {
            self::overlays::render_changelog(frame, markdown, *scroll)
        }
        OverlayView::RecurrenceHistory(state) => render_recurrence_history(frame, state),
        OverlayView::SyncStatus(state) => render_sync_status(frame, state),
        OverlayView::DatabaseStats { stats, scroll } => {
            render_database_stats(frame, stats, *scroll)
        }
        OverlayView::Update(state) => render_update(frame, state),
        OverlayView::Detail { .. } | OverlayView::AttachmentPreview { .. } => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn render_overlay(
    frame: &mut Frame,
    store: &TuiStore,
    widgets: &mut WidgetState,
    list: &mut ListSurface,
    overlay: &OverlayView,
    inline_title_editor: bool,
    focused_detail_target: Option<&crate::tui::app::DetailTargetId>,
    hovered_detail_target: Option<&crate::tui::app::DetailTargetId>,
    detail_expanded_sections: &std::collections::BTreeSet<crate::tui::app::DetailSection>,
    detail_text_selection: Option<&crate::tui::detail_selection::DetailTextSelection>,
    inline_images: Option<&DetailInlineImageContext>,
    pending_attachments: &[crate::tui::attachment_controller::PendingAttachmentView],
    removed_epic_child: Option<&crate::tui::app::RemovedEpicChild>,
) {
    if let OverlayView::AttachmentPreview { attachment_id, .. } = overlay {
        if let Some(item) = store.selected_task(list.selected_task()) {
            render_attachment_preview(frame, item, attachment_id, widgets, inline_images);
        }
        return;
    }
    if matches!(overlay, OverlayView::Detail { .. }) {
        let OverlayView::Detail { scroll } = overlay else {
            unreachable!();
        };
        render_detail_underlay(
            frame,
            store,
            widgets,
            list.selected_task(),
            *scroll,
            None,
            focused_detail_target,
            hovered_detail_target,
            detail_expanded_sections,
            detail_text_selection,
            inline_images,
            pending_attachments,
            removed_epic_child,
        );
        return;
    }
    if overlay_dims_underlay(overlay, inline_title_editor) {
        dialog::dim_rendered_background(frame);
    }
    if let OverlayView::DetailHelp { scroll } = overlay {
        render_detail_help(frame, *scroll, focused_detail_target);
        return;
    }
    render_overlay_content(frame, overlay, inline_title_editor);
}
