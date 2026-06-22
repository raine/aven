use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::query::TaskListItem;
use crate::tui::event::{Action, CommandSpec, KeySequence, ShortcutLookup, shortcut_label};
use crate::tui::overlay::{OverlayOutcome, OverlayState};
use crate::tui::store::SidebarEntry;
use crate::tui::ui::detail_scroll_cap;

const DETAIL_COMMANDS: &[CommandSpec] = &[
    CommandSpec::implemented(
        "detail-edit-title",
        "edit selected task title",
        "Edit",
        &[KeySequence {
            codes: &[KeyCode::Char('e'), KeyCode::Char('t')],
            label: "e t",
        }],
        Action::BeginEditTitle,
    ),
    CommandSpec::implemented(
        "detail-edit-description",
        "edit selected task description",
        "Edit",
        &[KeySequence {
            codes: &[KeyCode::Char('e'), KeyCode::Char('d')],
            label: "e d",
        }],
        Action::BeginEditDescription,
    ),
    CommandSpec::implemented(
        "detail-edit-project",
        "edit selected task project",
        "Edit",
        &[KeySequence {
            codes: &[KeyCode::Char('e'), KeyCode::Char('p')],
            label: "e p",
        }],
        Action::BeginEditProject,
    ),
    CommandSpec::implemented(
        "detail-edit-labels",
        "edit selected task labels",
        "Edit",
        &[
            KeySequence {
                codes: &[KeyCode::Char('l')],
                label: "l",
            },
            KeySequence {
                codes: &[KeyCode::Char('e'), KeyCode::Char('l')],
                label: "e l",
            },
        ],
        Action::BeginEditLabels,
    ),
    CommandSpec::implemented(
        "detail-add-note",
        "add a note to selected task",
        "Task detail",
        &[KeySequence {
            codes: &[KeyCode::Char('n')],
            label: "n",
        }],
        Action::BeginAddNote,
    ),
    CommandSpec::implemented(
        "detail-status-picker",
        "open status picker",
        "Status",
        &[KeySequence {
            codes: &[KeyCode::Char('s')],
            label: "s",
        }],
        Action::BeginStatusPicker,
    ),
    CommandSpec::implemented(
        "detail-status-done",
        "set status to done",
        "Status",
        &[KeySequence {
            codes: &[KeyCode::Char('d')],
            label: "d",
        }],
        Action::SetStatus("done"),
    ),
    CommandSpec::implemented(
        "detail-edit-priority",
        "edit selected task priority",
        "Priority",
        &[
            KeySequence {
                codes: &[KeyCode::Char('p')],
                label: "p",
            },
            KeySequence {
                codes: &[KeyCode::Char('e'), KeyCode::Char('r')],
                label: "e r",
            },
        ],
        Action::BeginEditPriority,
    ),
    CommandSpec::implemented(
        "detail-delete",
        "confirm deleting selected task",
        "Task detail",
        &[KeySequence {
            codes: &[KeyCode::Char('D')],
            label: "D",
        }],
        Action::Delete,
    ),
    CommandSpec::implemented(
        "detail-copy-ref",
        "copy selected task display ref",
        "Task detail",
        &[KeySequence {
            codes: &[KeyCode::Char('y')],
            label: "y",
        }],
        Action::CopyShortRef,
    ),
    CommandSpec::implemented(
        "detail-copy-id",
        "copy selected task id",
        "Task detail",
        &[KeySequence {
            codes: &[KeyCode::Char('Y')],
            label: "Y",
        }],
        Action::CopyDurableRef,
    ),
    CommandSpec::implemented(
        "detail-undo",
        "undo last TUI mutation",
        "General",
        &[KeySequence {
            codes: &[KeyCode::Char('u')],
            label: "u",
        }],
        Action::Undo,
    ),
];

pub(crate) fn handle_detail_overlay_key(
    key: KeyEvent,
    overlay: OverlayState,
    terminal_width: u16,
    terminal_height: u16,
    task: Option<&TaskListItem>,
) -> OverlayOutcome {
    let OverlayState::Detail { mut scroll } = overlay else {
        return OverlayOutcome::None(overlay);
    };
    let page = detail_page_scroll_rows(terminal_height);
    let scroll_cap = task
        .map(|task| detail_scroll_cap(task, terminal_width, terminal_height))
        .unwrap_or(0);
    scroll = scroll.min(scroll_cap);
    match key.code {
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => OverlayOutcome::Cancelled,
        KeyCode::Char('j') | KeyCode::Down => {
            scroll = scroll.saturating_add(1).min(scroll_cap);
            OverlayOutcome::None(OverlayState::Detail { scroll })
        }
        KeyCode::Char('k') | KeyCode::Up => {
            scroll = scroll.saturating_sub(1);
            OverlayOutcome::None(OverlayState::Detail { scroll })
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            scroll = scroll.saturating_add(page).min(scroll_cap);
            OverlayOutcome::None(OverlayState::Detail { scroll })
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            scroll = scroll.saturating_sub(page);
            OverlayOutcome::None(OverlayState::Detail { scroll })
        }
        KeyCode::PageDown => {
            scroll = scroll.saturating_add(page).min(scroll_cap);
            OverlayOutcome::None(OverlayState::Detail { scroll })
        }
        KeyCode::PageUp => {
            scroll = scroll.saturating_sub(page);
            OverlayOutcome::None(OverlayState::Detail { scroll })
        }
        _ => OverlayOutcome::None(OverlayState::Detail { scroll }),
    }
}

fn detail_page_scroll_rows(terminal_height: u16) -> u16 {
    terminal_height.saturating_sub(6).max(1)
}

pub(crate) fn detail_task_delta(key: KeyEvent) -> Option<isize> {
    if !key.modifiers.is_empty() {
        return None;
    }
    match key.code {
        KeyCode::Char(']') => Some(1),
        KeyCode::Char('[') => Some(-1),
        _ => None,
    }
}

pub(crate) enum DetailShortcut {
    Action(Action),
    Prefix,
    Missing(String),
}

pub(crate) fn detail_shortcut(sequence: &[KeyCode]) -> DetailShortcut {
    match crate::tui::event::resolve_shortcut_in(DETAIL_COMMANDS, sequence) {
        ShortcutLookup::Found(action) | ShortcutLookup::Ambiguous(action) => {
            DetailShortcut::Action(action)
        }
        ShortcutLookup::Prefix => DetailShortcut::Prefix,
        ShortcutLookup::Missing => DetailShortcut::Missing(shortcut_label(sequence)),
    }
}

pub(crate) fn next_index(
    selected: Option<usize>,
    len: usize,
    delta: isize,
    wrap: bool,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let current = selected.unwrap_or(0);
    let next = current as isize + delta;
    if (0..len as isize).contains(&next) {
        Some(next as usize)
    } else if wrap && delta > 0 {
        Some(0)
    } else if wrap && delta < 0 {
        Some(len - 1)
    } else {
        Some(current)
    }
}

pub(crate) fn next_selectable_sidebar(
    selected: Option<usize>,
    entries: &[SidebarEntry],
    delta: isize,
    wrap: bool,
) -> Option<usize> {
    if entries.is_empty() || entries.iter().all(|entry| entry.target.is_none()) {
        return None;
    }
    let mut index = selected.unwrap_or(0);
    for _ in 0..entries.len() {
        let next = index as isize + delta;
        index = if (0..entries.len() as isize).contains(&next) {
            next as usize
        } else if wrap && delta > 0 {
            0
        } else if wrap && delta < 0 {
            entries.len() - 1
        } else {
            index
        };
        if entries[index].target.is_some() {
            return Some(index);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::store::SidebarTarget;

    fn section(label: &str) -> SidebarEntry {
        SidebarEntry {
            label: label.to_string(),
            count: 0,
            target: None,
            section: true,
        }
    }

    fn item(label: &str) -> SidebarEntry {
        SidebarEntry {
            label: label.to_string(),
            count: 0,
            target: Some(SidebarTarget::All),
            section: false,
        }
    }

    #[test]
    fn wraps_up_from_first_sidebar_item_to_last_item() {
        let entries = [
            section("Smart Views"),
            item("All"),
            section("Projects"),
            item("APP app"),
        ];

        let selected = next_selectable_sidebar(Some(1), &entries, -1, true);

        assert_eq!(selected, Some(3));
    }

    #[test]
    fn wraps_down_from_last_sidebar_item_to_first_item() {
        let entries = [
            section("Smart Views"),
            item("All"),
            section("Projects"),
            item("APP app"),
        ];

        let selected = next_selectable_sidebar(Some(3), &entries, 1, true);

        assert_eq!(selected, Some(1));
    }

    #[test]
    fn wraps_up_from_first_task_to_last_task() {
        assert_eq!(next_index(Some(0), 3, -1, true), Some(2));
    }

    #[test]
    fn detail_down_scroll_stops_at_cap() {
        let OverlayOutcome::None(OverlayState::Detail { scroll }) = handle_detail_overlay_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            OverlayState::Detail { scroll: 4 },
            80,
            24,
            None,
        ) else {
            panic!("expected detail overlay");
        };

        assert_eq!(scroll, 0);
    }

    #[test]
    fn detail_up_scroll_moves_after_resisted_down_scroll() {
        let OverlayOutcome::None(OverlayState::Detail { scroll }) = handle_detail_overlay_key(
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
            OverlayState::Detail { scroll: 4 },
            80,
            24,
            None,
        ) else {
            panic!("expected detail overlay");
        };

        assert_eq!(scroll, 0);
    }
}
