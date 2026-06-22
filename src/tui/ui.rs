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
use ratatui::widgets::{Block, Paragraph};

use crate::tui::app::{Focus, WidgetState};
use crate::tui::overlay::{OverlayRoute, OverlayView, TextInputView};
use crate::tui::store::TuiStore;
use crate::tui::theme::{BG, FG};

#[derive(Clone)]
pub(crate) struct ViewState {
    pub(crate) focus: Focus,
    pub(crate) overlay: Option<OverlayView>,
    pub(crate) detail_underlay: bool,
    pub(crate) message: Option<String>,
    pub(crate) pending_shortcut: Vec<String>,
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

    if !view.pending_shortcut.is_empty()
        && !view
            .overlay
            .as_ref()
            .is_some_and(OverlayView::captures_input)
    {
        render_prefix_hints(frame, view);
    }
    if view.detail_underlay {
        render_detail_underlay(frame, store, widgets, detail_underlay_scroll(&view.overlay));
    }
    if let Some(overlay) = &view.overlay {
        render_overlay(frame, store, widgets, overlay);
    }
    if let Some(message) = &view.message {
        render_toast(frame, message);
    }
}

fn inline_title_editor(view: &ViewState) -> Option<&TextInputView> {
    if view.focus != Focus::Tasks {
        return None;
    }
    match &view.overlay {
        Some(OverlayView::TextInput(state)) if state.route == OverlayRoute::EditTitle => {
            Some(state)
        }
        _ => None,
    }
}

fn render_overlay_content(frame: &mut Frame, overlay: &OverlayView) {
    match overlay {
        OverlayView::Help { scroll } => render_help(frame, *scroll),
        OverlayView::DetailHelp { scroll } => render_detail_help(frame, *scroll),
        OverlayView::Search { input, cursor } => render_search(frame, input, *cursor),
        OverlayView::Command { input, cursor } => render_command(frame, input, *cursor),
        OverlayView::TextInput(state) if state.route == OverlayRoute::EditTitle => {}
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
        render_detail_underlay(frame, store, widgets, scroll);
        if matches!(overlay, OverlayView::DetailHelp { .. }) {
            render_overlay_content(frame, overlay);
        }
        return;
    }
    render_overlay_content(frame, overlay);
}
