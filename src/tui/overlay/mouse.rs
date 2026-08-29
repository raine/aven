use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Rect, Size};

use super::{
    AddTaskMode, HeaderMenuAction, OrderMenuState, OverlayOutcome, OverlayState, OverlaySubmit,
    RecurrenceHistoryAction, RecurrenceHistoryState, UpdateOverlayState,
    handle_generic_overlay_mouse,
};
use crate::tui::navigation::scroll_with_delta;
use crate::tui::store::TaskOrder;

#[derive(Debug, Clone, Copy)]
pub(crate) struct OverlayMouseContext {
    pub(crate) add_task_only: bool,
    pub(crate) detail_help_scroll_cap: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlayMouseOutcome {
    Retained(OverlayState),
    Closed,
    Cancelled,
    Submitted(OverlaySubmit),
    OpenUrl {
        overlay: OverlayState,
        url: String,
        error_context: &'static str,
    },
    OpenAddTaskControl(OverlayState),
    UpdateAction(UpdateOverlayState),
    RecurrenceHistoryAction {
        state: RecurrenceHistoryState,
        action: RecurrenceHistoryAction,
    },
    Warning {
        overlay: OverlayState,
        message: &'static str,
    },
}

pub(crate) fn dispatch_overlay_mouse(
    overlay: OverlayState,
    mouse: MouseEvent,
    terminal_size: Size,
    context: OverlayMouseContext,
) -> OverlayMouseOutcome {
    if matches!(
        mouse.kind,
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
    ) {
        return scroll_overlay(overlay, mouse.kind, terminal_size, context);
    }

    match overlay {
        OverlayState::RecurrenceHistory(state) => {
            dispatch_recurrence_history_mouse(*state, mouse, terminal_size)
        }
        OverlayState::Update(state) => dispatch_update_mouse(state, mouse, terminal_size),
        OverlayState::Changelog(state) => {
            if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                && let Some(url) = crate::tui::ui::changelog_link_at(
                    &state.markdown,
                    state.scroll,
                    terminal_size,
                    mouse.column,
                    mouse.row,
                )
            {
                return OverlayMouseOutcome::OpenUrl {
                    overlay: OverlayState::Changelog(state),
                    url,
                    error_context: "could not open changelog link",
                };
            }
            OverlayMouseOutcome::Retained(OverlayState::Changelog(state))
        }
        OverlayState::Command { mut state }
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) =>
        {
            let selected = state.highlighted.unwrap_or(0);
            let offset = selected.saturating_sub(7);
            let visible = state.candidates.len().saturating_sub(offset).min(8);
            let height = (visible as u16)
                .saturating_add(3)
                .saturating_add(u16::from(!state.candidates.is_empty()));
            let width = terminal_size.width.saturating_sub(2).min(112);
            let area = super::dialog_area(
                Rect::new(0, 0, terminal_size.width, terminal_size.height),
                width,
                height,
            );
            let first_row = area.y.saturating_add(2);
            if mouse.column >= area.x
                && mouse.column < area.right()
                && mouse.row >= first_row
                && usize::from(mouse.row.saturating_sub(first_row)) < visible
            {
                let index = offset + usize::from(mouse.row.saturating_sub(first_row));
                state.highlighted = Some(index);
            }
            OverlayMouseOutcome::Retained(OverlayState::Command { state })
        }
        OverlayState::HeaderMenu(state)
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) =>
        {
            let action = header_menu_action_at(&state, mouse.column, mouse.row, terminal_size);
            action.map_or(OverlayMouseOutcome::Closed, |action| {
                OverlayMouseOutcome::Submitted(OverlaySubmit::HeaderMenu { action })
            })
        }
        OverlayState::OrderMenu(state) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
            let order = order_menu_order_at(state, mouse.column, mouse.row, terminal_size);
            order.map_or(OverlayMouseOutcome::Closed, |order| {
                OverlayMouseOutcome::Submitted(OverlaySubmit::Order { order })
            })
        }
        OverlayState::AddTask(mut state)
            if state.mode == AddTaskMode::Compose
                && mouse.kind == MouseEventKind::Down(MouseButton::Left) =>
        {
            let field = crate::tui::ui::add_task_field_at(
                Rect::new(0, 0, terminal_size.width, terminal_size.height),
                context.add_task_only,
                crate::tui::ui::AddTaskLayout {
                    description: &state.description.lines,
                    mode: &state.mode,
                    has_attachments: !state.attachments.is_empty(),
                    show_schedule_error: state.schedule_error.is_some()
                        && state.schedule_validation_requested,
                },
                mouse.column,
                mouse.row,
            );
            let Some(field) = field else {
                return OverlayMouseOutcome::Retained(OverlayState::AddTask(state));
            };
            if !state.is_step_editable(field) {
                return OverlayMouseOutcome::Retained(OverlayState::AddTask(state));
            }
            state.focus = field;
            let overlay = OverlayState::AddTask(state);
            if field.is_metadata() {
                OverlayMouseOutcome::OpenAddTaskControl(overlay)
            } else {
                OverlayMouseOutcome::Retained(overlay)
            }
        }
        overlay => map_generic_outcome(handle_generic_overlay_mouse(overlay, mouse, terminal_size)),
    }
}

fn scroll_overlay(
    mut overlay: OverlayState,
    kind: MouseEventKind,
    terminal_size: Size,
    context: OverlayMouseContext,
) -> OverlayMouseOutcome {
    let delta = if kind == MouseEventKind::ScrollDown {
        1
    } else {
        -1
    };
    match &mut overlay {
        OverlayState::RecurrenceHistory(state) => state.move_selection(delta),
        OverlayState::Help { scroll } => {
            let cap = crate::tui::ui::help_scroll_cap(terminal_size.height);
            *scroll = scroll_with_delta(*scroll, delta, cap);
        }
        OverlayState::DetailHelp { scroll } => {
            *scroll = scroll_with_delta(*scroll, delta, context.detail_help_scroll_cap);
        }
        OverlayState::TextPanel(state) => {
            let cap = crate::tui::ui::text_panel_scroll_cap(&state.lines);
            state.scroll = scroll_with_delta(state.scroll, delta, cap);
        }
        OverlayState::Update(UpdateOverlayState::Available { notes, scroll, .. }) => {
            let cap = crate::tui::ui::update_notes_scroll_cap(notes, terminal_size);
            *scroll = scroll_with_delta(*scroll, delta, cap);
        }
        OverlayState::Changelog(state) => {
            let cap = crate::tui::changelog::changelog_scroll_cap(&state.markdown, terminal_size);
            state.scroll = scroll_with_delta(state.scroll, delta, cap);
        }
        _ => {}
    }
    OverlayMouseOutcome::Retained(overlay)
}

fn dispatch_update_mouse(
    state: UpdateOverlayState,
    mouse: MouseEvent,
    terminal_size: Size,
) -> OverlayMouseOutcome {
    if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
        return OverlayMouseOutcome::Retained(OverlayState::Update(state));
    }
    if let Some(url) =
        crate::tui::ui::update_link_at(&state, terminal_size, mouse.column, mouse.row)
    {
        return OverlayMouseOutcome::OpenUrl {
            overlay: OverlayState::Update(state),
            url,
            error_context: "could not open release-note link",
        };
    }
    let Some(action) =
        crate::tui::ui::update_action_at(&state, terminal_size, mouse.column, mouse.row)
    else {
        return OverlayMouseOutcome::Retained(OverlayState::Update(state));
    };
    let UpdateOverlayState::Available {
        plan,
        notes,
        scroll,
        cached,
        ..
    } = state
    else {
        return OverlayMouseOutcome::Retained(OverlayState::Update(state));
    };
    OverlayMouseOutcome::UpdateAction(UpdateOverlayState::Available {
        plan,
        notes,
        scroll,
        focus: action,
        cached,
    })
}

fn dispatch_recurrence_history_mouse(
    mut state: RecurrenceHistoryState,
    mouse: MouseEvent,
    terminal_size: Size,
) -> OverlayMouseOutcome {
    if !matches!(
        mouse.kind,
        MouseEventKind::Down(MouseButton::Left | MouseButton::Right)
    ) {
        return OverlayMouseOutcome::Retained(OverlayState::RecurrenceHistory(Box::new(state)));
    }
    let view = super::RecurrenceHistoryView::from_state(&state);
    let Some(index) =
        crate::tui::ui::recurrence_history_entry_at(&view, terminal_size, mouse.column, mouse.row)
    else {
        return OverlayMouseOutcome::Closed;
    };
    state.selected = Some(index);
    if mouse.kind == MouseEventKind::Down(MouseButton::Right) {
        let openable = state
            .selected_entry()
            .is_some_and(|entry| entry.openable && entry.task_id.is_some());
        if openable {
            return OverlayMouseOutcome::RecurrenceHistoryAction {
                state,
                action: RecurrenceHistoryAction::OpenTask,
            };
        }
        return OverlayMouseOutcome::Warning {
            overlay: OverlayState::RecurrenceHistory(Box::new(state)),
            message: "this history entry has no linked task",
        };
    }
    OverlayMouseOutcome::Retained(OverlayState::RecurrenceHistory(Box::new(state)))
}

fn header_menu_action_at(
    state: &super::HeaderMenuState,
    column: u16,
    row: u16,
    terminal_size: Size,
) -> Option<HeaderMenuAction> {
    let area = state.area(terminal_size.width, terminal_size.height);
    if !contains(area, column, row) {
        return None;
    }
    let item_index = row.saturating_sub(area.y).checked_sub(1)? as usize;
    state.items.get(item_index).map(|item| item.action.clone())
}

fn order_menu_order_at(
    state: OrderMenuState,
    column: u16,
    row: u16,
    terminal_size: Size,
) -> Option<TaskOrder> {
    let area = state.area(terminal_size.width, terminal_size.height);
    if !contains(area, column, row) {
        return None;
    }
    match row.saturating_sub(area.y) {
        1 => Some(TaskOrder::DueOn),
        2 => Some(TaskOrder::Created),
        3 => Some(TaskOrder::Updated),
        4 => Some(TaskOrder::Priority),
        5 => Some(TaskOrder::Project),
        6 => Some(TaskOrder::Title),
        _ => None,
    }
}

fn contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

fn map_generic_outcome(outcome: OverlayOutcome) -> OverlayMouseOutcome {
    match outcome {
        OverlayOutcome::None(overlay) => OverlayMouseOutcome::Retained(overlay),
        OverlayOutcome::Cancelled => OverlayMouseOutcome::Cancelled,
        OverlayOutcome::Submitted(submit) => OverlayMouseOutcome::Submitted(submit),
    }
}

#[cfg(test)]
mod tests {
    use aven_core::query::{RecurrenceHistoryEntry, RecurrenceHistoryKind, RecurrenceHistoryPage};
    use aven_core::recurrence::RecurrenceSeriesId;
    use chrono::Utc;
    use crossterm::event::{KeyModifiers, MouseEvent};

    use super::*;
    use crate::ids::WorkspaceId;
    use crate::tui::overlay::{HeaderMenuItem, HeaderMenuKind, HeaderMenuState};
    use crate::tui::store::TaskQuery;

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn context() -> OverlayMouseContext {
        OverlayMouseContext {
            add_task_only: false,
            detail_help_scroll_cap: 4,
        }
    }

    #[test]
    fn header_menu_click_submits_typed_action_and_outside_click_closes() {
        let state = HeaderMenuState {
            kind: HeaderMenuKind::View,
            column: 4,
            row: 0,
            selected: 0,
            items: vec![HeaderMenuItem {
                key: "i".to_string(),
                label: "inbox".to_string(),
                selected: false,
                action: HeaderMenuAction::View(TaskQuery::Inbox),
            }],
        };
        let area = state.area(80, 24);
        let outcome = dispatch_overlay_mouse(
            OverlayState::HeaderMenu(state.clone()),
            mouse(MouseEventKind::Down(MouseButton::Left), area.x, area.y + 1),
            Size::new(80, 24),
            context(),
        );
        assert!(matches!(
            outcome,
            OverlayMouseOutcome::Submitted(OverlaySubmit::HeaderMenu {
                action: HeaderMenuAction::View(TaskQuery::Inbox)
            })
        ));

        let outcome = dispatch_overlay_mouse(
            OverlayState::HeaderMenu(state),
            mouse(MouseEventKind::Down(MouseButton::Left), 0, 23),
            Size::new(80, 24),
            context(),
        );
        assert_eq!(outcome, OverlayMouseOutcome::Closed);
    }

    #[test]
    fn overlay_scroll_updates_owned_state_and_clamps_to_context_cap() {
        let mut overlay = OverlayState::DetailHelp { scroll: 3 };
        for _ in 0..3 {
            let OverlayMouseOutcome::Retained(next) = dispatch_overlay_mouse(
                overlay,
                mouse(MouseEventKind::ScrollDown, 0, 0),
                Size::new(80, 24),
                context(),
            ) else {
                panic!("detail help scroll should retain the overlay");
            };
            overlay = next;
        }
        assert_eq!(overlay, OverlayState::DetailHelp { scroll: 4 });
    }

    #[test]
    fn command_row_click_uses_catalog_order_for_highlight() {
        let state = crate::tui::overlay::CommandState::test_with_input("");
        let size = Size::new(100, 30);
        let outcome = dispatch_overlay_mouse(
            OverlayState::Command { state },
            mouse(MouseEventKind::Down(MouseButton::Left), 10, 11),
            size,
            context(),
        );
        assert!(matches!(
            outcome,
            OverlayMouseOutcome::Retained(OverlayState::Command { state })
                if state.highlighted_name() == Some("add-task")
        ));
    }

    #[test]
    fn recurrence_history_right_click_emits_feature_action() {
        let state = RecurrenceHistoryState::new(
            WorkspaceId::new(),
            RecurrenceSeriesId::new(),
            Utc::now(),
            RecurrenceHistoryPage {
                series_ref: "RCR-TEST".to_string(),
                items: vec![RecurrenceHistoryEntry {
                    kind: RecurrenceHistoryKind::Completed,
                    slot_on: Some("2026-08-01".to_string()),
                    interval_started_at: None,
                    interval_ended_at: None,
                    task_id: Some(crate::test_support::task_id("history-task")),
                    task_ref: Some("APP-TEST".to_string()),
                    openable: true,
                    archived_projection: false,
                    resolved_at: Some("2026-08-01T12:00:00Z".to_string()),
                }],
                offset: 0,
                limit: 10,
                total: 1,
                has_more: false,
            },
        );
        let size = Size::new(80, 24);
        let action = (0..size.height).find_map(|row| {
            (0..size.width).find_map(|column| {
                let outcome = dispatch_overlay_mouse(
                    OverlayState::RecurrenceHistory(Box::new(state.clone())),
                    mouse(MouseEventKind::Down(MouseButton::Right), column, row),
                    size,
                    context(),
                );
                matches!(
                    outcome,
                    OverlayMouseOutcome::RecurrenceHistoryAction {
                        action: RecurrenceHistoryAction::OpenTask,
                        ..
                    }
                )
                .then_some(outcome)
            })
        });
        assert!(action.is_some());
    }
}
