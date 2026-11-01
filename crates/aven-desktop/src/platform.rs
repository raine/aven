//! What GPUI does not do for itself.

/// `NSWorkspace.accessibilityDisplayShouldReduceMotion`.
#[cfg(target_os = "macos")]
pub fn reduce_motion() -> bool {
    use std::ffi::{CStr, c_void};

    unsafe extern "C" {
        fn objc_getClass(name: *const std::ffi::c_char) -> *mut c_void;
        fn sel_registerName(name: *const std::ffi::c_char) -> *mut c_void;
        fn objc_msgSend();
    }
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

#[cfg(not(target_os = "macos"))]
pub fn reduce_motion() -> bool {
    false
}

/// Takes the window off screen while it stays open, which `orderOut:` does and
/// `activate_window` undoes.
#[cfg(target_os = "macos")]
pub fn order_out(window: &gpui::Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::ffi::{CStr, c_void};

    unsafe extern "C" {
        fn sel_registerName(name: *const std::ffi::c_char) -> *mut c_void;
        fn objc_msgSend();
    }
    const WINDOW: &CStr = c"window";
    const ORDER_OUT: &CStr = c"orderOut:";
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: the handle holds this window's live `NSView`, both selectors exist
    // on AppKit's classes, and each `objc_msgSend` is cast to the signature of the
    // method it sends.
    unsafe {
        let native: extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let order: extern "C" fn(*mut c_void, *mut c_void, *mut c_void) =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let native = native(handle.ns_view.as_ptr(), sel_registerName(WINDOW.as_ptr()));
        order(
            native,
            sel_registerName(ORDER_OUT.as_ptr()),
            std::ptr::null_mut(),
        );
    }
}

#[cfg(not(target_os = "macos"))]
pub fn order_out(_: &gpui::Window) {}
