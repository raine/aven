mod clipboard;
mod editor;
mod gist;
mod terminal;
mod viewer;

#[cfg(test)]
pub(crate) use clipboard::clipboard_text_for_test;
pub(crate) use clipboard::{
    ClipboardImage, copy_to_clipboard, read_clipboard_image, read_clipboard_text,
};
#[cfg(test)]
pub(crate) use editor::fail_next_external_editor;
pub(crate) use editor::{
    configure_terminal_child_signals, edit_text_externally, is_editor_prefix_key,
};
pub(crate) use gist::create_secret_gist;
pub(crate) use terminal::{
    KeyboardEnhancementGuard, SuspendedTerminal, SystemTerminalTransition, TerminalTransition,
    detect_background,
};
#[cfg(test)]
pub(crate) use viewer::browser_url_for_test;
#[cfg(not(test))]
pub(crate) use viewer::open_image_in_default_viewer;
pub(crate) use viewer::open_url_in_default_browser;
