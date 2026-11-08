//! What GPUI does not do for itself, and the macOS services the app manages.

pub use aven_host::app_bundle::enclosing_bundle;
pub use aven_host::background;
pub use aven_host::daemon::service as agent_service;
/// Posts the app's notifications.
pub use aven_host::notify::un::InProcess as Notifier;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::ffi::{CStr, c_char, c_void};

unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> *mut c_void;
    fn sel_registerName(name: *const c_char) -> *mut c_void;
    fn objc_msgSend();
}

/// The executable the background agent runs while this app's installation
/// manages it.
pub fn agent_app() -> Option<std::path::PathBuf> {
    let layout = aven_host::desktop_install::Layout::current().ok()?;
    layout.manages_agent().then(|| layout.executable())
}

/// `NSWorkspace.accessibilityDisplayShouldReduceMotion`.
pub fn reduce_motion() -> bool {
    const CLASS: &CStr = c"NSWorkspace";
    const SHARED: &CStr = c"sharedWorkspace";
    const REDUCE: &CStr = c"accessibilityDisplayShouldReduceMotion";
    // SAFETY: both selectors exist on `NSWorkspace` since macOS 10.12, and each
    // `objc_msgSend` is cast to the signature of the method it sends.
    unsafe {
        let shared: extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let reduce: extern "C" fn(*mut c_void, *mut c_void) -> bool =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let workspace = shared(
            objc_getClass(CLASS.as_ptr()),
            sel_registerName(SHARED.as_ptr()),
        );
        reduce(workspace, sel_registerName(REDUCE.as_ptr()))
    }
}

/// `NSApplication.isActive`. A window can be closed while the app stays in front.
pub fn is_active() -> bool {
    const CLASS: &CStr = c"NSApplication";
    const SHARED: &CStr = c"sharedApplication";
    const ACTIVE: &CStr = c"isActive";
    // SAFETY: both selectors exist on `NSApplication`, and each `objc_msgSend` is
    // cast to the signature of the method it sends.
    unsafe {
        let shared: extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let active: extern "C" fn(*mut c_void, *mut c_void) -> bool =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let application = shared(
            objc_getClass(CLASS.as_ptr()),
            sel_registerName(SHARED.as_ptr()),
        );
        active(application, sel_registerName(ACTIVE.as_ptr()))
    }
}

/// Whether Option is held without Command or Control, read from the keyboard
/// whichever app is in front.
pub fn option_alone() -> bool {
    const CLASS: &CStr = c"NSEvent";
    const FLAGS: &CStr = c"modifierFlags";
    const CONTROL: usize = 1 << 18;
    const OPTION: usize = 1 << 19;
    const COMMAND: usize = 1 << 20;
    // SAFETY: `+[NSEvent modifierFlags]` exists since macOS 10.6, and
    // `objc_msgSend` is cast to its signature.
    let flags = unsafe {
        let flags: extern "C" fn(*mut c_void, *mut c_void) -> usize =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        flags(
            objc_getClass(CLASS.as_ptr()),
            sel_registerName(FLAGS.as_ptr()),
        )
    };
    flags & (OPTION | CONTROL | COMMAND) == OPTION
}

/// Takes the window off screen while it stays open, which `orderOut:` does and
/// `activate_window` undoes.
pub fn order_out(window: &gpui::Window) {
    const ORDER_OUT: &CStr = c"orderOut:";
    let Some((native, _)) = ns_window(window) else {
        return;
    };
    // SAFETY: `native` is this window's live `NSWindow`, `orderOut:` exists on
    // it, and `objc_msgSend` is cast to the method's signature.
    unsafe {
        let order: extern "C" fn(*mut c_void, *mut c_void, *mut c_void) =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        order(
            native,
            sel_registerName(ORDER_OUT.as_ptr()),
            std::ptr::null_mut(),
        );
    }
}

/// The family the app's text is set in: the system font.
pub fn ui_font() -> gpui::SharedString {
    ".SystemUIFont".into()
}

/// The room the window's own controls take at each end of the top bar.
pub struct TopBarInsets {
    pub start: gpui::Pixels,
    pub end: gpui::Pixels,
}

/// The traffic lights sit at the start of the top bar.
pub fn top_bar_insets(_window: &gpui::Window, _cx: &gpui::App) -> TopBarInsets {
    TopBarInsets {
        start: gpui::px(80.),
        end: gpui::px(0.),
    }
}

/// Does what the user's Dock settings say a double-clicked title bar does.
pub fn title_bar_double_click(window: &gpui::Window) {
    window.titlebar_double_click();
}

/// AppKit frames the window and draws its traffic lights.
pub fn frame(
    content: gpui::AnyElement,
    _window: &mut gpui::Window,
    _cx: &mut gpui::App,
) -> gpui::AnyElement {
    content
}

/// Leaves a transparent panel to draw its own frame and shadow, without the
/// title bar edge AppKit lights, and shows it at once without the utility
/// window's fade. Runs before the panel is shown: changing the style of a key
/// window takes its focus. Changing the style also moves first responder off
/// GPUI's view, which takes typing, so this hands it back.
pub fn plain_panel(window: &gpui::Window) {
    const SET_STYLE_MASK: &CStr = c"setStyleMask:";
    const SET_HAS_SHADOW: &CStr = c"setHasShadow:";
    const SET_ANIMATION: &CStr = c"setAnimationBehavior:";
    const MAKE_FIRST_RESPONDER: &CStr = c"makeFirstResponder:";
    /// `NSWindowStyleMaskBorderless | NSWindowStyleMaskNonactivatingPanel`.
    const BORDERLESS_PANEL: usize = 1 << 7;
    /// `NSWindowAnimationBehaviorNone`.
    const NO_ANIMATION: isize = 2;
    let Some((native, view)) = ns_window(window) else {
        return;
    };
    // SAFETY: `native` is this window's live `NSWindow` and `view` its content
    // view, the selectors exist on AppKit's classes, and each `objc_msgSend` is
    // cast to the signature of the method it sends.
    unsafe {
        let set_flag: extern "C" fn(*mut c_void, *mut c_void, bool) =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let set_behavior: extern "C" fn(*mut c_void, *mut c_void, isize) =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let set_mask: extern "C" fn(*mut c_void, *mut c_void, usize) =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        set_mask(
            native,
            sel_registerName(SET_STYLE_MASK.as_ptr()),
            BORDERLESS_PANEL,
        );
        set_flag(native, sel_registerName(SET_HAS_SHADOW.as_ptr()), false);
        set_behavior(
            native,
            sel_registerName(SET_ANIMATION.as_ptr()),
            NO_ANIMATION,
        );
        let make_first: extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> bool =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        make_first(
            native,
            sel_registerName(MAKE_FIRST_RESPONDER.as_ptr()),
            view,
        );
    }
}

/// The `NSWindow` of a GPUI window and its content `NSView`.
fn ns_window(window: &gpui::Window) -> Option<(*mut c_void, *mut c_void)> {
    const WINDOW: &CStr = c"window";
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    let view = handle.ns_view.as_ptr();
    // SAFETY: the handle holds this window's live `NSView`, which answers
    // `window`, and `objc_msgSend` is cast to that method's signature.
    let native = unsafe {
        let native: extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        native(view, sel_registerName(WINDOW.as_ptr()))
    };
    Some((native, view))
}

/// Opens `bundle` in the background once this process has exited, through Launch
/// Services, so it starts as an app and not as a child of this one, with the
/// settings that point this one at its data.
pub fn reopen_after_exit(bundle: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt as _;
    use std::process::{Command, Stdio};

    const SCRIPT: &str = r#"
        pid=$1
        shift
        while kill -0 "$pid" 2>/dev/null; do sleep 0.1; done
        exec /usr/bin/open -g "$@"
    "#;
    Command::new("/bin/sh")
        .args(["-c", SCRIPT, "aven-relaunch"])
        .arg(std::process::id().to_string())
        .args(aven_host::app_bundle::open_environment_args())
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map(drop)
}
