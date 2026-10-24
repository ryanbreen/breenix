//! High-level input events for Breengel applications, decoded from the window interface's
//! 320-byte `WindowEvent` (`docs/gui/breengel.md` section 19.1). Each field is read from its
//! own named field, never from another's, and a kind Breengel does not know arrives as
//! `Event::Unknown`.

use libbreenix::window::abi::{self, event_kind, modifier, WindowEvent};

/// Linux key codes (`KeyPress`/`KeyRelease` `keycode`) the apps match on.
pub mod keys {
    pub const ESC: u32 = 1;
    pub const MINUS: u32 = 12;
    pub const EQUAL: u32 = 13;
    pub const BACKSPACE: u32 = 14;
    pub const TAB: u32 = 15;
    pub const W: u32 = 17;
    pub const T: u32 = 20;
    pub const ENTER: u32 = 28;
    pub const HOME: u32 = 102;
    pub const UP: u32 = 103;
    pub const PAGE_UP: u32 = 104;
    pub const LEFT: u32 = 105;
    pub const RIGHT: u32 = 106;
    pub const END: u32 = 107;
    pub const DOWN: u32 = 108;
    pub const PAGE_DOWN: u32 = 109;
    pub const DELETE: u32 = 111;
}

/// Linux button codes (`MouseButton` `button`) the apps match on.
pub mod buttons {
    pub use libbreenix::window::abi::button::{LEFT, MIDDLE, RIGHT};
}

/// Modifier keys held when an event was made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Super (the logo key).
    pub logo: bool,
}

impl Modifiers {
    pub fn from_bits(bits: u32) -> Self {
        Self {
            shift: bits & modifier::SHIFT != 0,
            ctrl: bits & modifier::CTRL != 0,
            alt: bits & modifier::ALT != 0,
            logo: bits & modifier::SUPER != 0,
        }
    }
}

/// High-level input event.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A key was pressed. `keycode` is the Linux key code, `text` the UTF-8 text the compositor
    /// attached, `ascii` its first byte when that is printable ASCII (else 0), and `repeat`
    /// marks an auto-repeat.
    KeyPress { ascii: u8, keycode: u32, modifiers: Modifiers, text: String, repeat: bool },
    /// A key was released.
    KeyRelease { keycode: u32, modifiers: Modifiers },
    /// Text input, whole.
    Text { text: String },
    /// The pointer moved to window-local coordinates.
    MouseMove { x: i32, y: i32 },
    /// A pointer button (Linux button code: 0x110 left, 0x111 right, 0x112 middle) was pressed
    /// or released.
    MouseButton { button: u32, pressed: bool, x: i32, y: i32 },
    /// Scroll steps: `delta_y` > 0 scrolls up, `delta_x` > 0 right.
    Scroll { delta_x: i32, delta_y: i32 },
    /// This window gained keyboard focus.
    FocusGained,
    /// This window lost keyboard focus.
    FocusLost,
    /// The compositor asked this window to close.
    CloseRequested,
    /// The compositor asks the owner to resize its buffers and redraw.
    ResizeRequested { width: u32, height: u32 },
    /// The window's buffers now have this size.
    Resized { width: u32, height: u32 },
    /// A resize was refused; the window keeps this (old) size.
    ResizeFailed { width: u32, height: u32 },
    /// The system font configuration changed (made by Breengel's font watcher).
    FontChanged,
    /// A compositor claimed the display, with this epoch.
    CompositorChanged { epoch: u64 },
    /// The desktop clipboard changed; clipboard_get reads its text and change serial.
    ClipboardChanged,
    /// Another registered window replaced this window's clipboard ownership.
    ClipboardLost,
    /// The ring was full: this many events were lost before the next one.
    Overflow { lost: u32 },
    /// A kind Breengel does not know.
    Unknown { kind: u16 },
}

/// An event as `Window::dispatch` delivers it: its delivery serial, timestamp and
/// decoded event. Clipboard notices carry kernel delivery identities rather than
/// input serials; other kernel/Breengel events can have serial 0.
#[derive(Clone, Debug, PartialEq)]
pub struct Delivered {
    pub serial: u64,
    pub time_ns: u64,
    pub event: Event,
}

impl Event {
    /// Decode a window event. A kind outside the contract's table is `Unknown`, never a key.
    pub fn from_raw(raw: &WindowEvent) -> Self {
        let modifiers = Modifiers::from_bits(raw.modifiers);
        let text = String::from_utf8_lossy(raw.text()).into_owned();
        match raw.kind {
            event_kind::KEY_PRESS => Event::KeyPress {
                ascii: match raw.text().first() {
                    Some(&b) if (0x20..0x7f).contains(&b) || b == b'\n' || b == b'\t' || b == 0x08 => b,
                    _ => 0,
                },
                keycode: raw.keycode,
                modifiers,
                text,
                repeat: raw.flags & abi::EVENT_REPEAT != 0,
            },
            event_kind::KEY_RELEASE => Event::KeyRelease { keycode: raw.keycode, modifiers },
            event_kind::TEXT => Event::Text { text },
            event_kind::POINTER_MOVE => Event::MouseMove { x: raw.x, y: raw.y },
            event_kind::POINTER_BUTTON => Event::MouseButton {
                button: raw.button,
                pressed: raw.pressed != 0,
                x: raw.x,
                y: raw.y,
            },
            event_kind::SCROLL => Event::Scroll { delta_x: raw.scroll_x, delta_y: raw.scroll_y },
            event_kind::FOCUS_GAINED => Event::FocusGained,
            event_kind::FOCUS_LOST => Event::FocusLost,
            event_kind::CLOSE_REQUESTED => Event::CloseRequested,
            event_kind::RESIZE_REQUESTED => Event::ResizeRequested { width: raw.width, height: raw.height },
            event_kind::RESIZED => Event::Resized { width: raw.width, height: raw.height },
            event_kind::RESIZE_FAILED => Event::ResizeFailed { width: raw.width, height: raw.height },
            event_kind::FONT_CHANGED => Event::FontChanged,
            event_kind::COMPOSITOR_CHANGED => Event::CompositorChanged { epoch: raw.epoch },
            event_kind::CLIPBOARD_CHANGED => Event::ClipboardChanged,
            event_kind::CLIPBOARD_LOST => Event::ClipboardLost,
            event_kind::OVERFLOW => Event::Overflow { lost: raw.lost },
            kind => Event::Unknown { kind },
        }
    }
}

impl Delivered {
    pub fn from_raw(raw: &WindowEvent) -> Self {
        Delivered { serial: raw.serial, time_ns: raw.time_ns, event: Event::from_raw(raw) }
    }
}
