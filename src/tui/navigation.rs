use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::query::TaskListItem;
use crate::tui::event::{
    Action, CommandContext, ShortcutLookup, resolve_shortcut_for, shortcut_label,
};
use crate::tui::overlay::{OverlayOutcome, OverlayState};
use crate::tui::store::SidebarEntry;
use crate::tui::ui::detail_scroll_cap;

pub(crate) fn scroll_with_delta(scroll: u16, delta: isize, cap: u16) -> u16 {
    let scroll = scroll.min(cap);
    let next = scroll as isize + delta;
    if next < 0 {
        0
    } else if next > cap as isize {
        cap
    } else {
        next as u16
    }
}

pub(crate) fn detail_scroll_with_delta(
    scroll: u16,
    delta: isize,
    terminal_width: u16,
    terminal_height: u16,
    task: Option<&TaskListItem>,
) -> u16 {
    let cap = task
        .map(|task| detail_scroll_cap(task, terminal_width, terminal_height))
        .unwrap_or(0);
    scroll_with_delta(scroll, delta, cap)
}

pub(crate) fn handle_detail_overlay_key(
    key: KeyEvent,
    overlay: OverlayState,
    terminal_width: u16,
    terminal_height: u16,
    task: Option<&TaskListItem>,
) -> OverlayOutcome {
    let OverlayState::Detail { scroll } = overlay else {
        return OverlayOutcome::None(overlay);
    };
    let scroll = detail_scroll_with_delta(scroll, 0, terminal_width, terminal_height, task);
    let page = detail_page_scroll_rows(terminal_height);
    match key.code {
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => OverlayOutcome::Cancelled,
        KeyCode::Char('j') | KeyCode::Down => OverlayOutcome::None(OverlayState::Detail {
            scroll: detail_scroll_with_delta(scroll, 1, terminal_width, terminal_height, task),
        }),
        KeyCode::Char('k') | KeyCode::Up => OverlayOutcome::None(OverlayState::Detail {
            scroll: detail_scroll_with_delta(scroll, -1, terminal_width, terminal_height, task),
        }),
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            OverlayOutcome::None(OverlayState::Detail {
                scroll: detail_scroll_with_delta(
                    scroll,
                    page as isize,
                    terminal_width,
                    terminal_height,
                    task,
                ),
            })
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            OverlayOutcome::None(OverlayState::Detail {
                scroll: detail_scroll_with_delta(
                    scroll,
                    -(page as isize),
                    terminal_width,
                    terminal_height,
                    task,
                ),
            })
        }
        KeyCode::PageDown => OverlayOutcome::None(OverlayState::Detail {
            scroll: detail_scroll_with_delta(
                scroll,
                page as isize,
                terminal_width,
                terminal_height,
                task,
            ),
        }),
        KeyCode::PageUp => OverlayOutcome::None(OverlayState::Detail {
            scroll: detail_scroll_with_delta(
                scroll,
                -(page as isize),
                terminal_width,
                terminal_height,
                task,
            ),
        }),
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
    match resolve_shortcut_for(CommandContext::Detail, sequence) {
        ShortcutLookup::Found(action) => DetailShortcut::Action(action),
        ShortcutLookup::Ambiguous(action) if sequence.len() > 1 => DetailShortcut::Action(action),
        ShortcutLookup::Ambiguous(_) | ShortcutLookup::Prefix => DetailShortcut::Prefix,
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
    use crate::tui::store::{SidebarEntryTarget, TaskView};

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
            target: Some(SidebarEntryTarget::View(TaskView::Queue)),
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

    #[test]
    fn scroll_with_delta_caps_to_range() {
        assert_eq!(scroll_with_delta(2, 3, 5), 5);
        assert_eq!(scroll_with_delta(2, -1, 5), 1);
        assert_eq!(scroll_with_delta(2, -3, 5), 0);
        assert_eq!(scroll_with_delta(8, 5, 7), 7);
    }

    #[test]
    fn ignored_detail_keys_clamp_stale_scroll_to_cap() {
        let OverlayOutcome::None(OverlayState::Detail { scroll }) = handle_detail_overlay_key(
            KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
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
