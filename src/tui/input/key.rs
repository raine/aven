use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::event::{Action, CommandCatalog, CommandHandler};
use crate::tui::shortcut_buffer::{NormalShortcutResolution, ShortcutBuffer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImagePasteTarget {
    Detail,
    AddTask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeyInput {
    Action(Action),
    PasteImage(ImagePasteTarget),
    FooterChoice(KeyEvent),
    CancelShortcut,
    ToggleHelp,
    ScrollPrefix(isize),
    Overlay(KeyEvent),
    Normal(KeyCode),
    Ignore,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct KeyRouteState {
    pub(crate) footer_choice: bool,
    pub(crate) shortcut_pending: bool,
    pub(crate) prefix_hints: bool,
    pub(crate) overlay_captures: bool,
    pub(crate) detail_overlay: bool,
    pub(crate) add_task_image_target: bool,
}

pub(crate) fn route_key(key: KeyEvent, state: KeyRouteState, terminal_height: u16) -> KeyInput {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return KeyInput::Action(Action::Quit);
    }
    if is_image_paste_key(key) && state.detail_overlay {
        return KeyInput::PasteImage(ImagePasteTarget::Detail);
    }
    if is_image_paste_key(key) && state.add_task_image_target {
        return KeyInput::PasteImage(ImagePasteTarget::AddTask);
    }
    if state.footer_choice {
        return KeyInput::FooterChoice(key);
    }
    if key.code == KeyCode::Esc && state.shortcut_pending {
        return KeyInput::CancelShortcut;
    }
    if key.code == KeyCode::Char(':')
        && matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
        && !state.shortcut_pending
        && (!state.overlay_captures || state.detail_overlay)
    {
        return KeyInput::Action(Action::BeginCommand);
    }
    if state.overlay_captures {
        if key.code == KeyCode::Char('?') && state.detail_overlay {
            return KeyInput::ToggleHelp;
        }
        if let Some(delta) = prefix_scroll_delta(key, terminal_height, state.prefix_hints) {
            return KeyInput::ScrollPrefix(delta);
        }
        return KeyInput::Overlay(key);
    }
    if key.code == KeyCode::Char('?') {
        return KeyInput::ToggleHelp;
    }
    if let Some(delta) = prefix_scroll_delta(key, terminal_height, state.prefix_hints) {
        return KeyInput::ScrollPrefix(delta);
    }
    if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
        return KeyInput::Normal(key.code);
    }
    KeyInput::Ignore
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NormalKeyTranslation {
    pub(crate) shortcut: ShortcutBuffer,
    pub(crate) input: NormalKeyInput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NormalKeyInput {
    Overlay(KeyEvent),
    CancelShortcut,
    CancelOverlay,
    Command(CommandHandler),
    Prefix,
    Missing(String),
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn route_normal_key(
    shortcut: &ShortcutBuffer,
    code: KeyCode,
    overlay_captures: bool,
    catalog: &CommandCatalog,
) -> NormalKeyTranslation {
    route_normal_key_in_domain(
        shortcut,
        code,
        overlay_captures,
        catalog,
        crate::tui::event::RoutingDomain::Normal,
    )
}

pub(crate) fn route_normal_key_in_domain(
    shortcut: &ShortcutBuffer,
    code: KeyCode,
    overlay_captures: bool,
    catalog: &CommandCatalog,
    domain: crate::tui::event::RoutingDomain,
) -> NormalKeyTranslation {
    let mut shortcut = shortcut.clone();
    let input = if overlay_captures && (code != KeyCode::Esc || shortcut.is_empty()) {
        NormalKeyInput::Overlay(KeyEvent::new(code, KeyModifiers::NONE))
    } else if code == KeyCode::Esc {
        if shortcut.cancel() {
            NormalKeyInput::CancelShortcut
        } else {
            NormalKeyInput::CancelOverlay
        }
    } else {
        match shortcut.resolve_normal_in_domain(code, catalog, domain) {
            NormalShortcutResolution::Command(handler) => NormalKeyInput::Command(handler),
            NormalShortcutResolution::Prefix => NormalKeyInput::Prefix,
            NormalShortcutResolution::Missing(label) => NormalKeyInput::Missing(label),
        }
    };
    NormalKeyTranslation { shortcut, input }
}

fn is_image_paste_key(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('v')
        && key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
}

fn prefix_scroll_delta(key: KeyEvent, terminal_height: u16, active: bool) -> Option<isize> {
    if !active || (!key.modifiers.is_empty() && key.modifiers != KeyModifiers::SHIFT) {
        return None;
    }
    match key.code {
        KeyCode::Down => Some(1),
        KeyCode::Up => Some(-1),
        KeyCode::PageDown => Some(terminal_height.saturating_sub(4).max(1) as isize),
        KeyCode::PageUp => Some(-(terminal_height.saturating_sub(4).max(1) as isize)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn control_c_routes_to_quit_before_overlay_capture() {
        let input = route_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            KeyRouteState {
                overlay_captures: true,
                ..KeyRouteState::default()
            },
            24,
        );
        assert_eq!(input, KeyInput::Action(Action::Quit));
    }

    #[test]
    fn command_panel_routes_before_detail_overlay_capture() {
        let input = route_key(
            KeyEvent::new(KeyCode::Char(':'), KeyModifiers::SHIFT),
            KeyRouteState {
                overlay_captures: true,
                detail_overlay: true,
                ..KeyRouteState::default()
            },
            24,
        );
        assert_eq!(input, KeyInput::Action(Action::BeginCommand));
    }

    #[test]
    fn pending_escape_cancels_the_shortcut() {
        let input = route_key(
            key(KeyCode::Esc),
            KeyRouteState {
                shortcut_pending: true,
                overlay_captures: true,
                ..KeyRouteState::default()
            },
            24,
        );
        assert_eq!(input, KeyInput::CancelShortcut);
    }

    #[test]
    fn prefix_page_scroll_uses_terminal_height() {
        let input = route_key(
            key(KeyCode::PageDown),
            KeyRouteState {
                prefix_hints: true,
                ..KeyRouteState::default()
            },
            18,
        );
        assert_eq!(input, KeyInput::ScrollPrefix(14));
    }

    #[test]
    fn detail_help_routes_before_detail_input_capture() {
        let input = route_key(
            key(KeyCode::Char('?')),
            KeyRouteState {
                overlay_captures: true,
                detail_overlay: true,
                ..KeyRouteState::default()
            },
            24,
        );
        assert_eq!(input, KeyInput::ToggleHelp);
    }

    #[test]
    fn normal_shortcut_translation_returns_action_and_next_buffer() {
        let mut shortcut = ShortcutBuffer::default();
        let catalog = CommandCatalog::default();
        shortcut.resolve_normal_in_domain(
            KeyCode::Char('g'),
            &catalog,
            crate::tui::event::RoutingDomain::Normal,
        );
        let translation = route_normal_key(&shortcut, KeyCode::Char('g'), false, &catalog);
        assert_eq!(
            translation.input,
            NormalKeyInput::Command(CommandHandler::BuiltIn(Action::First))
        );
        assert!(translation.shortcut.is_empty());
    }

    #[test]
    fn image_paste_routes_to_the_active_feature() {
        let input = route_key(
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::SUPER),
            KeyRouteState {
                add_task_image_target: true,
                ..KeyRouteState::default()
            },
            24,
        );
        assert_eq!(input, KeyInput::PasteImage(ImagePasteTarget::AddTask));
    }
}
