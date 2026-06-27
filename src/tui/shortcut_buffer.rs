use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::event::{Action, ShortcutLookup, key_label, resolve_shortcut, shortcut_label};
use crate::tui::navigation::{DetailShortcut, detail_shortcut};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NormalShortcutResolution {
    Action(Action),
    Prefix,
    Missing(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DetailShortcutResolution {
    Action(Action),
    Prefix,
    MissingAfterPrefix(String),
    PassThrough,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ShortcutBuffer {
    codes: Vec<KeyCode>,
}

impl ShortcutBuffer {
    pub(crate) fn is_empty(&self) -> bool {
        self.codes.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.codes.clear();
    }

    pub(crate) fn cancel(&mut self) -> bool {
        let had_pending = !self.codes.is_empty();
        self.clear();
        had_pending
    }

    pub(crate) fn labels(&self) -> Vec<String> {
        self.codes.iter().map(|code| key_label(*code)).collect()
    }

    pub(crate) fn resolve_normal(&mut self, code: KeyCode) -> NormalShortcutResolution {
        let had_pending = !self.codes.is_empty();
        let sequence = self.with_code(code);
        match resolve_shortcut(&sequence) {
            ShortcutLookup::Found(action) => {
                self.clear();
                NormalShortcutResolution::Action(action)
            }
            ShortcutLookup::Ambiguous(_) if !had_pending => {
                self.codes = sequence;
                NormalShortcutResolution::Prefix
            }
            ShortcutLookup::Ambiguous(action) => {
                self.clear();
                NormalShortcutResolution::Action(action)
            }
            ShortcutLookup::Prefix => {
                self.codes = sequence;
                NormalShortcutResolution::Prefix
            }
            ShortcutLookup::Missing => {
                let label = shortcut_label(&sequence);
                self.clear();
                NormalShortcutResolution::Missing(label)
            }
        }
    }

    pub(crate) fn resolve_detail(&mut self, key: KeyEvent) -> DetailShortcutResolution {
        if !key.modifiers.is_empty() {
            return DetailShortcutResolution::PassThrough;
        }

        let had_pending = !self.codes.is_empty();
        let sequence = self.with_code(key.code);
        match detail_shortcut(&sequence) {
            DetailShortcut::Action(action) => {
                self.clear();
                DetailShortcutResolution::Action(action)
            }
            DetailShortcut::Prefix => {
                self.codes = sequence;
                DetailShortcutResolution::Prefix
            }
            DetailShortcut::Missing(label) if had_pending => {
                self.clear();
                DetailShortcutResolution::MissingAfterPrefix(label)
            }
            DetailShortcut::Missing(_) => DetailShortcutResolution::PassThrough,
        }
    }

    pub(crate) fn begin_editor_prefix(&mut self) {
        self.codes.clear();
        self.codes.push(KeyCode::Char('x'));
    }

    pub(crate) fn begin_add_task_status_prefix(&mut self) {
        self.codes.clear();
        self.codes.push(KeyCode::Char('t'));
    }

    pub(crate) fn has_add_task_status_prefix(&self) -> bool {
        self.codes == [KeyCode::Char('t')]
    }

    pub(crate) fn take_add_task_status_request(&mut self, key: KeyEvent) -> Option<&'static str> {
        if self.codes != [KeyCode::Char('t')] || !key.modifiers.is_empty() {
            return None;
        }

        let status = match key.code {
            KeyCode::Char('i') => Some("inbox"),
            KeyCode::Char('b') => Some("backlog"),
            KeyCode::Char('t') => Some("todo"),
            KeyCode::Char('a') => Some("active"),
            KeyCode::Char('d') => Some("done"),
            KeyCode::Char('x') => Some("canceled"),
            _ => None,
        };
        if status.is_some() || key.code == KeyCode::Esc {
            self.clear();
        }
        status
    }

    pub(crate) fn begin_add_task_priority_prefix(&mut self) {
        self.codes.clear();
        self.codes.push(KeyCode::Char('r'));
    }

    pub(crate) fn has_add_task_priority_prefix(&self) -> bool {
        self.codes == [KeyCode::Char('r')]
    }

    pub(crate) fn take_add_task_priority_request(&mut self, key: KeyEvent) -> Option<&'static str> {
        if self.codes != [KeyCode::Char('r')] || !key.modifiers.is_empty() {
            return None;
        }

        let priority = match key.code {
            KeyCode::Char('n') => Some("none"),
            KeyCode::Char('l') => Some("low"),
            KeyCode::Char('m') => Some("medium"),
            KeyCode::Char('h') => Some("high"),
            KeyCode::Char('u') => Some("urgent"),
            _ => None,
        };
        if priority.is_some() || key.code == KeyCode::Esc {
            self.clear();
        }
        priority
    }

    pub(crate) fn take_editor_open_request(&mut self, key: KeyEvent) -> bool {
        if self.codes != [KeyCode::Char('x')] {
            return false;
        }

        self.clear();
        key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('e')
    }

    fn with_code(&self, code: KeyCode) -> Vec<KeyCode> {
        let mut sequence = self.codes.clone();
        sequence.push(code);
        sequence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_prefix_is_stored_and_rendered() {
        let mut buffer = ShortcutBuffer::default();
        assert_eq!(
            buffer.resolve_normal(KeyCode::Char('t')),
            NormalShortcutResolution::Prefix
        );
        assert_eq!(buffer.labels(), vec!["t".to_string()]);
    }

    #[test]
    fn normal_missing_clears_and_reports_full_label() {
        let mut buffer = ShortcutBuffer::default();
        assert_eq!(
            buffer.resolve_normal(KeyCode::Char('t')),
            NormalShortcutResolution::Prefix
        );
        assert_eq!(
            buffer.resolve_normal(KeyCode::Char('z')),
            NormalShortcutResolution::Missing("t z".to_string())
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn detail_missing_without_prefix_passes_through() {
        let mut buffer = ShortcutBuffer::default();
        assert_eq!(
            buffer.resolve_detail(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
            DetailShortcutResolution::PassThrough
        );
    }

    #[test]
    fn detail_missing_after_prefix_clears_and_warns() {
        let mut buffer = ShortcutBuffer::default();
        assert_eq!(
            buffer.resolve_detail(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE)),
            DetailShortcutResolution::Prefix
        );
        assert_eq!(
            buffer.resolve_detail(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE)),
            DetailShortcutResolution::MissingAfterPrefix("t z".to_string())
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn editor_prefix_non_open_key_clears_and_returns_false() {
        let mut buffer = ShortcutBuffer::default();
        buffer.begin_editor_prefix();
        assert!(
            !buffer
                .take_editor_open_request(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE,))
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn editor_prefix_ctrl_e_clears_and_returns_true() {
        let mut buffer = ShortcutBuffer::default();
        buffer.begin_editor_prefix();
        assert!(
            buffer
                .take_editor_open_request(
                    KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL,)
                )
        );
        assert!(buffer.is_empty());
    }
}
