//! A system-wide shortcut through Carbon's hot keys, which reach the app while
//! another one is in front and need no accessibility permission. Linux has none.
// The Carbon bindings stay compiled on Linux, where nothing calls them.
#![cfg_attr(target_os = "linux", allow(dead_code, unused_imports))]
use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::OnceLock;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// A key with modifiers, as a person recorded it. `key` is GPUI's name for
/// the key, such as `k`, `space` or `f5`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Shortcut {
    pub key: String,
    pub cmd: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Shortcut {
    pub fn from_keystroke(keystroke: &gpui::Keystroke) -> Self {
        let modifiers = keystroke.modifiers;
        Self {
            key: keystroke.key.to_lowercase(),
            cmd: modifiers.platform,
            ctrl: modifiers.control,
            alt: modifiers.alt,
            shift: modifiers.shift,
        }
    }

    /// Whether the shortcut leaves ordinary typing alone: it needs Command,
    /// Control or Option, and a key the hot key table knows.
    pub fn usable(&self) -> bool {
        (self.cmd || self.ctrl || self.alt) && key_code(&self.key).is_some()
    }

    /// The shortcut as macOS menus write it, such as `⌥⌘Space`.
    pub fn label(&self) -> String {
        let mut label = String::new();
        for (on, glyph) in [
            (self.ctrl, '⌃'),
            (self.alt, '⌥'),
            (self.shift, '⇧'),
            (self.cmd, '⌘'),
        ] {
            if on {
                label.push(glyph);
            }
        }
        let key = match self.key.as_str() {
            "space" => "Space".to_string(),
            "enter" => "↩".to_string(),
            "tab" => "⇥".to_string(),
            "backspace" => "⌫".to_string(),
            "escape" => "⎋".to_string(),
            "left" => "←".to_string(),
            "right" => "→".to_string(),
            "up" => "↑".to_string(),
            "down" => "↓".to_string(),
            key => key.to_uppercase(),
        };
        label.push_str(&key);
        label
    }

    fn carbon_modifiers(&self) -> u32 {
        const CMD: u32 = 0x100;
        const SHIFT: u32 = 0x200;
        const OPTION: u32 = 0x800;
        const CONTROL: u32 = 0x1000;
        [
            (self.cmd, CMD),
            (self.shift, SHIFT),
            (self.alt, OPTION),
            (self.ctrl, CONTROL),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .fold(0, |mask, (_, bit)| mask | bit)
    }
}

/// The ANSI keyboard's virtual key code for `key`. Hot keys name physical
/// keys, so other layouts match by position.
fn key_code(key: &str) -> Option<u32> {
    const LETTERS: [u32; 26] = [
        0, 11, 8, 2, 14, 3, 5, 4, 34, 38, 40, 37, 46, 45, 31, 35, 12, 15, 1, 17, 32, 9, 13, 7, 16,
        6,
    ];
    const DIGITS: [u32; 10] = [29, 18, 19, 20, 21, 23, 22, 26, 28, 25];
    const FUNCTIONS: [u32; 12] = [122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111];
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_lowercase() {
            return Some(LETTERS[(c as u8 - b'a') as usize]);
        }
        if c.is_ascii_digit() {
            return Some(DIGITS[(c as u8 - b'0') as usize]);
        }
    }
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<usize>().ok()) {
        return (1..=12).contains(&n).then(|| FUNCTIONS[n - 1]);
    }
    Some(match key {
        "=" => 24,
        "-" => 27,
        "]" => 30,
        "[" => 33,
        "'" => 39,
        ";" => 41,
        "\\" => 42,
        "," => 43,
        "/" => 44,
        "." => 47,
        "`" => 50,
        "enter" => 36,
        "tab" => 48,
        "space" => 49,
        "backspace" => 51,
        "escape" => 53,
        "left" => 123,
        "right" => 124,
        "down" => 125,
        "up" => 126,
        _ => return None,
    })
}

#[repr(C)]
struct EventTypeSpec {
    class: u32,
    kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

type Handler = extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32;

#[cfg(target_os = "macos")]
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> *mut c_void;
    fn InstallEventHandler(
        target: *mut c_void,
        handler: Handler,
        count: u32,
        types: *const EventTypeSpec,
        data: *mut c_void,
        out: *mut *mut c_void,
    ) -> i32;
    fn RegisterEventHotKey(
        code: u32,
        modifiers: u32,
        id: EventHotKeyId,
        target: *mut c_void,
        options: u32,
        out: *mut *mut c_void,
    ) -> i32;
    fn UnregisterEventHotKey(hot_key: *mut c_void) -> i32;
}

const KEYBOARD_EVENTS: u32 = u32::from_be_bytes(*b"keyb");
const HOT_KEY_PRESSED: u32 = 5;
const SIGNATURE: u32 = u32::from_be_bytes(*b"AVEN");
/// Fails registration when another app holds the same keys.
const EXCLUSIVE: u32 = 1;
const HOT_KEY_EXISTS: i32 = -9878;

static PRESSES: OnceLock<UnboundedSender<()>> = OnceLock::new();

extern "C" fn pressed(_: *mut c_void, _: *mut c_void, _: *mut c_void) -> i32 {
    if let Some(presses) = PRESSES.get() {
        let _ = presses.send(());
    }
    0
}

/// Starts hearing hot keys and returns their presses. Only the first call
/// listens. Runs on the main thread, whose event loop delivers them.
#[cfg(target_os = "macos")]
pub fn listen() -> Option<UnboundedReceiver<()>> {
    let (presses, received) = unbounded_channel();
    PRESSES.set(presses).ok()?;
    let spec = EventTypeSpec {
        class: KEYBOARD_EVENTS,
        kind: HOT_KEY_PRESSED,
    };
    let mut handler = null_mut();
    // SAFETY: the application target lives as long as the app, `spec` outlives
    // the call, and `pressed` matches Carbon's handler signature.
    let status = unsafe {
        InstallEventHandler(
            GetApplicationEventTarget(),
            pressed,
            1,
            &spec,
            null_mut(),
            &mut handler,
        )
    };
    (status == 0).then_some(received)
}

/// A registered shortcut, which frees its keys when dropped.
#[cfg(target_os = "macos")]
pub struct HotKey(*mut c_void);

#[cfg(target_os = "macos")]
impl Drop for HotKey {
    fn drop(&mut self) {
        // SAFETY: the reference came from a successful registration and is
        // unregistered only here.
        unsafe {
            UnregisterEventHotKey(self.0);
        }
    }
}

/// Takes `shortcut` for the app, or says why it cannot.
#[cfg(target_os = "macos")]
pub fn register(shortcut: &Shortcut) -> Result<HotKey, String> {
    let code = key_code(&shortcut.key).filter(|_| shortcut.usable());
    let Some(code) = code else {
        return Err(format!("{} can\u{2019}t be a shortcut", shortcut.label()));
    };
    let id = EventHotKeyId {
        signature: SIGNATURE,
        id: 1,
    };
    let mut hot_key = null_mut();
    // SAFETY: the application target lives as long as the app and `hot_key`
    // outlives the call.
    let status = unsafe {
        RegisterEventHotKey(
            code,
            shortcut.carbon_modifiers(),
            id,
            GetApplicationEventTarget(),
            EXCLUSIVE,
            &mut hot_key,
        )
    };
    match status {
        0 => Ok(HotKey(hot_key)),
        HOT_KEY_EXISTS => Err(format!("Another app already uses {}", shortcut.label())),
        status => Err(format!(
            "macOS refused {} (error {status})",
            shortcut.label()
        )),
    }
}

#[cfg(target_os = "linux")]
pub fn listen() -> Option<UnboundedReceiver<()>> {
    None
}

#[cfg(target_os = "linux")]
pub enum HotKey {}

#[cfg(target_os = "linux")]
pub fn register(shortcut: &Shortcut) -> Result<HotKey, String> {
    Err(format!(
        "Aven has no system-wide shortcuts on Linux, so {} can\u{2019}t be one",
        shortcut.label()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shortcut(key: &str) -> Shortcut {
        Shortcut {
            key: key.to_string(),
            ..Shortcut::default()
        }
    }

    #[test]
    fn shortcuts_need_a_modifier_and_a_known_key() {
        assert!(!shortcut("k").usable());
        let shifted = Shortcut {
            shift: true,
            ..shortcut("k")
        };
        assert!(!shifted.usable(), "Shift alone still types");
        let held = Shortcut {
            alt: true,
            ..shortcut("space")
        };
        assert!(held.usable());
        let unknown = Shortcut {
            cmd: true,
            ..shortcut("ä")
        };
        assert!(!unknown.usable());
    }

    #[test]
    fn keys_map_to_ansi_positions() {
        assert_eq!(key_code("a"), Some(0));
        assert_eq!(key_code("k"), Some(40));
        assert_eq!(key_code("z"), Some(6));
        assert_eq!(key_code("0"), Some(29));
        assert_eq!(key_code("9"), Some(25));
        assert_eq!(key_code("f12"), Some(111));
        assert_eq!(key_code("f13"), None);
        assert_eq!(key_code("space"), Some(49));
    }

    #[test]
    fn labels_follow_menu_order() {
        let all = Shortcut {
            key: "k".to_string(),
            cmd: true,
            ctrl: true,
            alt: true,
            shift: true,
        };
        assert_eq!(all.label(), "⌃⌥⇧⌘K");
        assert_eq!(all.carbon_modifiers(), 0x1b00);
    }
}
