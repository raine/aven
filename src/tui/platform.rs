mod clipboard;
mod editor;
mod gist;
mod terminal;
mod viewer;

pub(crate) use crate::clipboard::read_text as read_clipboard_text;
#[cfg(test)]
pub(crate) use clipboard::clipboard_text_for_test;
pub(crate) use clipboard::{ClipboardImage, copy_to_clipboard, read_clipboard_image};
#[cfg(test)]
pub(crate) use editor::fail_next_external_editor;
pub(crate) use editor::{
    configure_terminal_child_signals, edit_text_externally, is_editor_prefix_key,
};
pub(crate) use gist::create_secret_gist;
pub(crate) use terminal::{
    KeyboardEnhancementGuard, SuspendedTerminal, SystemTerminalTransition, TerminalTransition,
    detect_appearance,
};
#[cfg(test)]
pub(crate) use viewer::browser_url_for_test;
#[cfg(not(test))]
pub(crate) use viewer::open_image_in_default_viewer;
pub(crate) use viewer::open_url_in_default_browser;
