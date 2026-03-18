//! High-level input events for Breengel applications.

use libbreenix::graphics::{WindowInputEvent, input_event_type};

/// Modifier key bitmask.
#[derive(Clone, Copy, Debug, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl Modifiers {
    fn from_raw(bits: u16) -> Self {
        Self {
            shift: bits & 1 != 0,
            ctrl: bits & 2 != 0,
            alt: bits & 4 != 0,
        }
    }
}

/// High-level input event.
#[derive(Clone, Debug)]
pub enum Event {
    /// A key was pressed. `ascii` is the ASCII value (0 if not printable).
    /// `keycode` is the raw USB HID keycode.
    KeyPress { ascii: u8, keycode: u16, modifiers: Modifiers },
    /// A key was released.
    KeyRelease { keycode: u16, modifiers: Modifiers },
    /// Mouse moved to window-local coordinates.
    MouseMove { x: i32, y: i32 },
    /// Mouse button pressed or released.
    MouseButton { button: u8, pressed: bool, x: i32, y: i32 },
    /// Mouse scroll wheel event.
    ///
    /// `delta_y` > 0 means scroll up (content moves down / offset decreases).
    /// `delta_y` < 0 means scroll down (content moves up / offset increases).
    Scroll { delta_y: i32 },
    /// This window gained keyboard focus.
    FocusGained,
    /// This window lost keyboard focus.
    FocusLost,
    /// The window manager requested this window be closed.
    CloseRequested,
    /// The window was resized by the window manager. The buffer has already
    /// been reallocated — update application layout state as needed.
    Resized { width: u32, height: u32 },
    /// The system font configuration changed. The new font is already loaded
    /// internally — call `win.take_mono_font()` to get it. Recalculate text
    /// metrics, grid dimensions, etc. as needed.
    FontChanged,
}

impl Event {
    /// Convert a raw kernel input event to a high-level Event.
    ///
    /// Unknown event types fall back to a `KeyPress` with ascii=0 and
    /// the raw keycode, so they are not silently dropped.
    pub fn from_raw(raw: &WindowInputEvent) -> Self {
        match raw.event_type {
            input_event_type::KEY_PRESS => Event::KeyPress {
                ascii: raw.mouse_x as u8,
                keycode: raw.keycode,
                modifiers: Modifiers::from_raw(raw.modifiers),
            },
            input_event_type::KEY_RELEASE => Event::KeyRelease {
                keycode: raw.keycode,
                modifiers: Modifiers::from_raw(raw.modifiers),
            },
            input_event_type::MOUSE_MOVE => Event::MouseMove {
                x: raw.mouse_x as i32,
                y: raw.mouse_y as i32,
            },
            input_event_type::MOUSE_BUTTON => Event::MouseButton {
                button: raw.keycode as u8,
                pressed: raw.scroll_y != 0,
                x: raw.mouse_x as i32,
                y: raw.mouse_y as i32,
            },
            input_event_type::MOUSE_SCROLL => Event::Scroll {
                delta_y: raw.scroll_y as i32,
            },
            input_event_type::FOCUS_GAINED => Event::FocusGained,
            input_event_type::FOCUS_LOST => Event::FocusLost,
            input_event_type::CLOSE_REQUESTED => Event::CloseRequested,
            input_event_type::WINDOW_RESIZED => Event::Resized {
                width: raw.keycode as u32,
                height: raw.mouse_x as u16 as u32,
            },
            _ => Event::KeyPress {
                ascii: 0,
                keycode: raw.keycode,
                modifiers: Modifiers::default(),
            },
        }
    }
}
