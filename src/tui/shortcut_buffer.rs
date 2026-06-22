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
        let sequence = self.with_code(code);
        match resolve_shortcut(&sequence) {
            ShortcutLookup::Found(action) | ShortcutLookup::Ambiguous(action) => {
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
            buffer.resolve_normal(KeyCode::Char('m')),
            NormalShortcutResolution::Prefix
        );
        assert_eq!(buffer.labels(), vec!["m".to_string()]);
    }

    #[test]
    fn normal_missing_clears_and_reports_full_label() {
        let mut buffer = ShortcutBuffer::default();
        assert_eq!(
            buffer.resolve_normal(KeyCode::Char('m')),
            NormalShortcutResolution::Prefix
        );
        assert_eq!(
            buffer.resolve_normal(KeyCode::Char('z')),
            NormalShortcutResolution::Missing("m z".to_string())
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
            buffer.resolve_detail(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE)),
            DetailShortcutResolution::Prefix
        );
        assert_eq!(
            buffer.resolve_detail(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE)),
            DetailShortcutResolution::MissingAfterPrefix("e z".to_string())
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
