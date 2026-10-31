//! The one system setting GPUI does not read for itself.

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
