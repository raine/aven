use crossterm::event::KeyCode;

use crate::mutation::status_for_key;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Quit,
    MoveDown,
    MoveUp,
    First,
    Last,
    ToggleFocus,
    ToggleDetail,
    ToggleHelp,
    BeginSearch,
    AcceptSearch,
    CancelSearch,
    BackspaceSearch,
    SearchChar(char),
    Refresh,
    CycleSort,
    SetStatus(&'static str),
    CyclePriority(bool),
    Delete,
    Restore,
    None,
}

impl Action {
    pub(crate) fn from_search_key(code: KeyCode) -> Self {
        match code {
            KeyCode::Esc => Self::CancelSearch,
            KeyCode::Enter => Self::AcceptSearch,
            KeyCode::Backspace => Self::BackspaceSearch,
            KeyCode::Char(ch) => Self::SearchChar(ch),
            _ => Self::None,
        }
    }

    pub(crate) fn from_normal_key(code: KeyCode) -> Self {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => Self::Quit,
            KeyCode::Char('j') | KeyCode::Down => Self::MoveDown,
            KeyCode::Char('k') | KeyCode::Up => Self::MoveUp,
            KeyCode::Char('g') | KeyCode::Home => Self::First,
            KeyCode::Char('G') | KeyCode::End => Self::Last,
            KeyCode::Tab | KeyCode::BackTab => Self::ToggleFocus,
            KeyCode::Enter | KeyCode::Char('l') => Self::ToggleDetail,
            KeyCode::Char('?') => Self::ToggleHelp,
            KeyCode::Char('/') => Self::BeginSearch,
            KeyCode::Char('r') => Self::Refresh,
            KeyCode::Char('s') => Self::CycleSort,
            KeyCode::Char('p') => Self::CyclePriority(false),
            KeyCode::Char('P') => Self::CyclePriority(true),
            KeyCode::Char('d') => Self::Delete,
            KeyCode::Char('u') => Self::Restore,
            KeyCode::Char(ch) => status_for_key(ch)
                .map(Self::SetStatus)
                .unwrap_or(Self::None),
            _ => Self::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_navigation_keys() {
        assert_eq!(
            Action::from_normal_key(KeyCode::Char('j')),
            Action::MoveDown
        );
        assert_eq!(Action::from_normal_key(KeyCode::Down), Action::MoveDown);
        assert_eq!(Action::from_normal_key(KeyCode::Char('k')), Action::MoveUp);
        assert_eq!(Action::from_normal_key(KeyCode::Up), Action::MoveUp);
    }

    #[test]
    fn maps_status_keys() {
        assert_eq!(
            Action::from_normal_key(KeyCode::Char('1')),
            Action::SetStatus("inbox")
        );
        assert_eq!(
            Action::from_normal_key(KeyCode::Char('4')),
            Action::SetStatus("active")
        );
        assert_eq!(
            Action::from_normal_key(KeyCode::Char('6')),
            Action::SetStatus("canceled")
        );
    }

    #[test]
    fn search_mode_captures_text_keys() {
        assert_eq!(
            Action::from_search_key(KeyCode::Char('q')),
            Action::SearchChar('q')
        );
        assert_eq!(
            Action::from_search_key(KeyCode::Enter),
            Action::AcceptSearch
        );
        assert_eq!(Action::from_search_key(KeyCode::Esc), Action::CancelSearch);
    }
}
