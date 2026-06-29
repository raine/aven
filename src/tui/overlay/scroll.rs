use crossterm::event::{KeyCode, KeyEvent};

use crate::tui::navigation::scroll_with_delta;

pub(crate) struct ScrollState {
    pub(crate) scroll: u16,
    pub(crate) cap: u16,
}

pub(crate) enum ScrollKeyOutcome {
    Cancelled,
    Continue(ScrollState),
    Ignored,
}

pub(crate) fn handle_scroll_key(
    key: KeyEvent,
    state: ScrollState,
    close_keys: &[KeyCode],
    page_rows: u16,
) -> ScrollKeyOutcome {
    if close_keys.contains(&key.code) {
        return ScrollKeyOutcome::Cancelled;
    }

    let delta = match key.code {
        KeyCode::Char('j') | KeyCode::Down => Some(1),
        KeyCode::Char('k') | KeyCode::Up => Some(-1),
        KeyCode::PageDown => Some(page_rows as isize),
        KeyCode::PageUp => Some(-(page_rows as isize)),
        _ => None,
    };

    match delta {
        Some(delta) => ScrollKeyOutcome::Continue(ScrollState {
            scroll: scroll_with_delta(state.scroll, delta, state.cap),
            cap: state.cap,
        }),
        None => ScrollKeyOutcome::Ignored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn close_key_cancels() {
        let result = handle_scroll_key(
            key(KeyCode::Esc),
            ScrollState { scroll: 5, cap: 10 },
            &[KeyCode::Esc, KeyCode::Enter],
            0,
        );
        assert!(matches!(result, ScrollKeyOutcome::Cancelled));
    }

    #[test]
    fn j_key_scrolls_down() {
        let result = handle_scroll_key(
            key(KeyCode::Char('j')),
            ScrollState { scroll: 0, cap: 10 },
            &[KeyCode::Esc],
            0,
        );
        assert!(matches!(result, ScrollKeyOutcome::Continue(s) if s.scroll == 1));
    }

    #[test]
    fn k_key_scrolls_up() {
        let result = handle_scroll_key(
            key(KeyCode::Char('k')),
            ScrollState { scroll: 3, cap: 10 },
            &[KeyCode::Esc],
            0,
        );
        assert!(matches!(result, ScrollKeyOutcome::Continue(s) if s.scroll == 2));
    }

    #[test]
    fn ignored_key_returns_ignored() {
        let result = handle_scroll_key(
            key(KeyCode::Char('z')),
            ScrollState { scroll: 0, cap: 10 },
            &[KeyCode::Esc],
            0,
        );
        assert!(matches!(result, ScrollKeyOutcome::Ignored));
    }

    #[test]
    fn scroll_caps_at_limit() {
        let result = handle_scroll_key(
            key(KeyCode::Char('j')),
            ScrollState {
                scroll: 10,
                cap: 10,
            },
            &[KeyCode::Esc],
            0,
        );
        assert!(matches!(result, ScrollKeyOutcome::Continue(s) if s.scroll == 10));
    }

    #[test]
    fn page_down_with_zero_rows_is_ignored() {
        let result = handle_scroll_key(
            key(KeyCode::PageDown),
            ScrollState { scroll: 0, cap: 10 },
            &[KeyCode::Esc],
            0,
        );
        assert!(matches!(result, ScrollKeyOutcome::Continue(s) if s.scroll == 0));
    }
}
