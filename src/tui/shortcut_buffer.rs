use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::event::{
    CatalogShortcutLookup, CommandCatalog, CommandHandler, key_label, shortcut_label,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NormalShortcutResolution {
    Command(CommandHandler),
    Prefix,
    Missing(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DetailShortcutResolution {
    Action(crate::tui::event::Action),
    Custom(usize),
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

    pub(crate) fn resolve_normal_in_domain(
        &mut self,
        code: KeyCode,
        catalog: &CommandCatalog,
        domain: crate::tui::event::RoutingDomain,
    ) -> NormalShortcutResolution {
        let had_pending = !self.codes.is_empty();
        let sequence = self.with_code(code);
        match catalog.resolve_shortcut_in_domain(domain, &sequence) {
            CatalogShortcutLookup::Found(handler) => {
                self.clear();
                NormalShortcutResolution::Command(handler)
            }
            CatalogShortcutLookup::Ambiguous(_) if !had_pending => {
                self.codes = sequence;
                NormalShortcutResolution::Prefix
            }
            CatalogShortcutLookup::Ambiguous(handler) => {
                self.clear();
                NormalShortcutResolution::Command(handler)
            }
            CatalogShortcutLookup::Prefix => {
                self.codes = sequence;
                NormalShortcutResolution::Prefix
            }
            CatalogShortcutLookup::Missing => {
                let label = shortcut_label(&sequence);
                self.clear();
                NormalShortcutResolution::Missing(label)
            }
        }
    }

    pub(crate) fn resolve_detail_in_domain(
        &mut self,
        key: KeyEvent,
        catalog: &CommandCatalog,
        domain: crate::tui::event::RoutingDomain,
    ) -> DetailShortcutResolution {
        self.resolve_detail_with_section(key, catalog, domain, None)
    }

    pub(crate) fn resolve_detail_in_focus(
        &mut self,
        key: KeyEvent,
        catalog: &CommandCatalog,
        section: crate::tui::app::DetailSection,
    ) -> DetailShortcutResolution {
        self.resolve_detail_with_section(
            key,
            catalog,
            crate::tui::event::RoutingDomain::DetailRelated,
            Some(section),
        )
    }

    fn resolve_detail_with_section(
        &mut self,
        key: KeyEvent,
        catalog: &CommandCatalog,
        domain: crate::tui::event::RoutingDomain,
        section: Option<crate::tui::app::DetailSection>,
    ) -> DetailShortcutResolution {
        if !key.modifiers.is_empty() && key.modifiers != KeyModifiers::SHIFT {
            return DetailShortcutResolution::PassThrough;
        }

        let had_pending = !self.codes.is_empty();
        let sequence = self.with_code(key.code);
        match catalog.resolve_shortcut_in_context(domain, section, &sequence) {
            CatalogShortcutLookup::Found(handler) => {
                self.clear();
                detail_resolution(handler)
            }
            CatalogShortcutLookup::Ambiguous(_) if !had_pending => {
                self.codes = sequence;
                DetailShortcutResolution::Prefix
            }
            CatalogShortcutLookup::Ambiguous(handler) => {
                self.clear();
                detail_resolution(handler)
            }
            CatalogShortcutLookup::Prefix => {
                self.codes = sequence;
                DetailShortcutResolution::Prefix
            }
            CatalogShortcutLookup::Missing if had_pending => {
                self.clear();
                DetailShortcutResolution::MissingAfterPrefix(shortcut_label(&sequence))
            }
            CatalogShortcutLookup::Missing => DetailShortcutResolution::PassThrough,
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

fn detail_resolution(handler: CommandHandler) -> DetailShortcutResolution {
    match handler {
        CommandHandler::BuiltIn(action) => DetailShortcutResolution::Action(action),
        CommandHandler::Custom(command_id) => DetailShortcutResolution::Custom(command_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_prefix_is_stored_and_rendered() {
        let mut buffer = ShortcutBuffer::default();
        assert_eq!(
            buffer.resolve_normal_in_domain(
                KeyCode::Char('t'),
                &CommandCatalog::default(),
                crate::tui::event::RoutingDomain::Normal
            ),
            NormalShortcutResolution::Prefix
        );
        assert_eq!(buffer.labels(), vec!["t".to_string()]);
    }

    #[test]
    fn normal_missing_clears_and_reports_full_label() {
        let mut buffer = ShortcutBuffer::default();
        let catalog = CommandCatalog::default();
        assert_eq!(
            buffer.resolve_normal_in_domain(
                KeyCode::Char('t'),
                &catalog,
                crate::tui::event::RoutingDomain::Normal
            ),
            NormalShortcutResolution::Prefix
        );
        assert_eq!(
            buffer.resolve_normal_in_domain(
                KeyCode::Char('z'),
                &catalog,
                crate::tui::event::RoutingDomain::Normal
            ),
            NormalShortcutResolution::Missing("t z".to_string())
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn detail_missing_without_prefix_passes_through() {
        let mut buffer = ShortcutBuffer::default();
        assert_eq!(
            buffer.resolve_detail_in_domain(
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
                &CommandCatalog::default(),
                crate::tui::event::RoutingDomain::DetailParent,
            ),
            DetailShortcutResolution::PassThrough
        );
    }

    #[test]
    fn detail_missing_after_prefix_clears_and_warns() {
        let mut buffer = ShortcutBuffer::default();
        let catalog = CommandCatalog::default();
        assert_eq!(
            buffer.resolve_detail_in_domain(
                KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
                &catalog,
                crate::tui::event::RoutingDomain::DetailParent,
            ),
            DetailShortcutResolution::Prefix
        );
        assert_eq!(
            buffer.resolve_detail_in_domain(
                KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
                &catalog,
                crate::tui::event::RoutingDomain::DetailParent,
            ),
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
