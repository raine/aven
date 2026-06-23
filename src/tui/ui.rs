mod detail;
mod dialog;
mod footer;
mod header;
mod input;
mod overlays;
mod shortcuts;
mod sidebar;
mod task_display;
mod task_list;
mod toast;
mod truncate;

use self::detail::render_detail_underlay;
use self::footer::{FooterMode, footer_bar};
use self::header::render_header;
use self::overlays::{
    render_confirm, render_multiline_input, render_picker, render_search, render_text_input,
    render_text_panel,
};
use self::shortcuts::{render_command, render_detail_help, render_help, render_prefix_hints};
use self::sidebar::{render_sidebar, render_sidebar_overlay};
use self::task_list::render_tasks;
use self::toast::render_toast;

pub(crate) use self::detail::detail_scroll_cap;
pub(crate) use self::shortcuts::{detail_help_scroll_cap, help_scroll_cap};

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Paragraph};

use crate::tui::app::{Focus, LoadingState, WidgetState};
use crate::tui::overlay::{OverlayRoute, OverlayView, TextInputView};
use crate::tui::store::TuiStore;
use crate::tui::theme::{BG, FG};
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
    pub(crate) message: Option<Toast>,
    pub(crate) pending_shortcut: Vec<String>,
    pub(crate) surface: ViewSurface,
    pub(crate) loading: Option<LoadingState>,
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
        if view.focus == Focus::Sidebar {
            render_sidebar_overlay(frame, store, widgets, view.focus, body);
        }
    } else {
        let [sidebar, main] =
            Layout::horizontal([Constraint::Max(26), Constraint::Fill(1)]).areas(body);
        render_sidebar(frame, store, widgets, view.focus, sidebar, false);
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
    if !view.pending_shortcut.is_empty() {
        render_prefix_hints(frame, view);
    }
    if let Some(toast) = &view.message {
        render_toast(frame, toast);
    }
    if let Some(loading) = &view.loading {
        render_loading(frame, loading);
    }
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
        render_add_task_surface_overlay(frame, overlay);
    }
    if !view.pending_shortcut.is_empty() {
        render_prefix_hints(frame, view);
    }
    if let Some(toast) = &view.message {
        render_toast(frame, toast);
    }
    if let Some(loading) = &view.loading {
        render_loading(frame, loading);
    }
}

fn render_loading(frame: &mut Frame, loading: &LoadingState) {
    let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let frame_symbol = frames[loading.frame() % frames.len()];
    render_toast(
        frame,
        &Toast::new(
            format!("{frame_symbol} {}", loading.message),
            crate::tui::toast::ToastSeverity::Info,
        ),
    );
}

fn render_add_task_surface_overlay(frame: &mut Frame, overlay: &OverlayView) {
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

fn render_overlay_content(frame: &mut Frame, overlay: &OverlayView, inline_title_editor: bool) {
    match overlay {
        OverlayView::Help { scroll } => render_help(frame, *scroll),
        OverlayView::DetailHelp { scroll } => render_detail_help(frame, *scroll),
        OverlayView::Search { input, cursor } => render_search(frame, input, *cursor),
        OverlayView::Command { input, cursor } => render_command(frame, input, *cursor),
        OverlayView::AddTask(state) => self::overlays::render_add_task(frame, state),
        OverlayView::TextInput(state)
            if state.route == OverlayRoute::EditTitle && inline_title_editor => {}
        OverlayView::TextInput(state) => render_text_input(frame, state),
        OverlayView::MultilineInput(state) => render_multiline_input(frame, state),
        OverlayView::Picker(state) => render_picker(frame, state),
        OverlayView::Confirm(state) => render_confirm(frame, state),
        OverlayView::TextPanel(state) => render_text_panel(frame, state),
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
