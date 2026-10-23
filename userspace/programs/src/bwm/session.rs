//! bwm's side of the window interface, version 2 (`docs/gui/breengel.md` sections 17 and
//! 19): the compositor claim, the registered windows, their geometry, the events bwm writes
//! to them, the scene descriptor it sleeps on, and the input it reads itself (the pointer and
//! the keyboard state of syscall 413, section 17.6).

use core::sync::atomic::{AtomicU64, Ordering};

pub use libbreenix::graphics::{key_state, mouse_pos, mouse_state_with_scroll, take_over_display};
pub use libbreenix::window::abi::cursor_shape;
use libbreenix::error::Error;
use libbreenix::errno::Errno;
use libbreenix::window::{self, abi};
use abi::{op, Plain};

/// A registered client window, as `LIST` reports it.
#[derive(Clone, Copy)]
pub struct WindowInfo {
    pub id: u64,
    pub owner_pid: u32,
    pub width: u32,
    pub height: u32,
    pub title_len: u32,
    pub title: [u8; 64],
}

/// The serial of the last event bwm wrote (section 19.3: strictly increasing, not reused).
static EVENT_SERIAL: AtomicU64 = AtomicU64::new(0);

/// Claim the compositor role: returns the claim's epoch.
pub fn claim() -> Result<u64, Error> {
    window::claim_compositor()
}

/// The scene descriptor (`EVENT_FD` of window 0): readable when what the next composite shows
/// may have changed.
pub fn scene_fd() -> Result<i32, Error> {
    window::call(op::EVENT_FD, &mut abi::WinId { id: 0 }).map(|fd| fd as i32)
}

/// Mark the scene descriptor's changes read.
pub fn drain_scene(fd: i32) {
    let mut changes = [0u8; 8];
    let _ = libbreenix::io::read(libbreenix::types::Fd::from_raw(fd as u64), &mut changes);
}

/// The registered windows into `out`, which is cleared first.
pub fn list_windows(out: &mut Vec<WindowInfo>) -> Result<(), Error> {
    out.clear();
    let mut infos = vec![abi::WinInfo::ZERO; 32];
    loop {
        let total = window::list(&mut infos)?;
        if total <= infos.len() {
            infos.truncate(total);
            break;
        }
        infos = vec![abi::WinInfo::ZERO; total + 8];
    }
    out.extend(infos.iter().filter(|i| i.id != 0).map(|i| WindowInfo {
        id: i.id,
        owner_pid: i.owner,
        width: i.width,
        height: i.height,
        title_len: i.title_len.min(64),
        title: i.title,
    }));
    Ok(())
}

/// Place window `id`'s content at `(x, y)`, `z` from the bottom, shown or minimized.
pub fn set_geometry(id: u64, x: i32, y: i32, z: u32, visible: bool) -> Result<(), Error> {
    let flags = if visible { abi::geometry_flags::VISIBLE } else { abi::geometry_flags::MINIMIZED };
    window::set_geometry(id, x, y, z, flags).map(|_| ())
}

/// What bwm tells a client window. Pointer positions are window-local.
#[derive(Clone, Copy)]
pub enum Input {
    /// A key from bwm's stdin parser: `ascii` its character (0 when it has no character) and `keycode` the
    /// parser's code (the USB HID usage for Enter, Backspace, Tab and the arrows, the
    /// control code for Ctrl+letter, the character otherwise).
    Key { ascii: u8, keycode: u16, modifiers: u16, repeat: bool },
    Move { x: i32, y: i32 },
    /// `button` 1 left, 2 right, 3 middle.
    Button { button: u32, pressed: bool, x: i32, y: i32 },
    Scroll { x: i32, y: i32, delta_y: i32 },
    FocusGained,
    FocusLost,
    Close,
    Overflow { lost: u32 },
    Resize { width: u32, height: u32 },
}

/// Write one event to window `id`, with the next input serial.
pub fn send(id: u64, input: Input) -> Result<usize, Error> {
    use abi::event_kind as kind;
    let mut event = abi::WindowEvent::ZERO;
    match input {
        Input::Key { ascii, keycode, modifiers, repeat } => {
            event.kind = kind::KEY_PRESS;
            event.modifiers = modifiers as u32;
            // The event carries a Linux key code and, on a press, the text (section 19.1).
            let (code, shift, text) = linux_key(ascii, keycode);
            event.keycode = code;
            if shift {
                event.modifiers |= abi::modifier::SHIFT;
            }
            if let Some(t) = text {
                event.text[0] = t;
                event.text_len = 1;
            }
            if repeat {
                event.flags |= abi::EVENT_REPEAT;
            }
        }
        Input::Move { x, y } => {
            event.kind = kind::POINTER_MOVE;
            (event.x, event.y) = (x, y);
        }
        Input::Button { button, pressed, x, y } => {
            event.kind = kind::POINTER_BUTTON;
            event.button = match button {
                1 => abi::button::LEFT,
                2 => abi::button::RIGHT,
                3 => abi::button::MIDDLE,
                _ => return Err(Error::Os(Errno::EINVAL)),
            };
            event.pressed = u32::from(pressed);
            (event.x, event.y) = (x, y);
        }
        Input::Scroll { x, y, delta_y } => {
            event.kind = kind::SCROLL;
            (event.x, event.y) = (x, y);
            event.scroll_y = delta_y;
        }
        Input::FocusGained => event.kind = kind::FOCUS_GAINED,
        Input::FocusLost => event.kind = kind::FOCUS_LOST,
        Input::Close => event.kind = kind::CLOSE_REQUESTED,
        Input::Overflow { lost } => { event.kind = kind::OVERFLOW; event.lost = lost; }
        Input::Resize { width, height } => {
            event.kind = kind::RESIZE_REQUESTED;
            event.width = width;
            event.height = height;
        }
    }
    event.serial = EVENT_SERIAL.fetch_add(1, Ordering::Relaxed) + 1;
    event.time_ns = libbreenix::time::now_monotonic().map_or(0, |t| t.as_nanos() as u64);
    window::write_events(id, &[event])
}

/// The Linux key code, whether Shift produced it, and the text of a key from bwm's stdin
/// parser (see `Input::Key`).
fn linux_key(ascii: u8, code: u16) -> (u32, bool, Option<u8>) {
    match (ascii, code) {
        (13, 0x28) => return (28, false, Some(b'\n')),
        (8, 0x2A) => return (14, false, Some(0x08)),
        (9, 0x2B) => return (15, false, Some(b'\t')),
        (0, 0x4F) => return (106, false, None),
        (0, 0x50) => return (105, false, None),
        (0, 0x51) => return (108, false, None),
        (0, 0x52) => return (103, false, None),
        (0x1b, _) => return (1, false, Some(0x1b)),
        _ => {}
    }
    // Ctrl+letter arrives as its control code.
    let c = if (1..=26).contains(&ascii) { b'a' + ascii - 1 } else { ascii };
    const LETTERS: [u32; 26] = [
        30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44,
    ];
    let (code, shift) = match c {
        b'a'..=b'z' => (LETTERS[(c - b'a') as usize], false),
        b'A'..=b'Z' => (LETTERS[(c - b'A') as usize], true),
        b'1'..=b'9' => (2 + (c - b'1') as u32, false),
        b'0' => (11, false),
        b' ' => (57, false),
        b'-' => (12, false),
        b'=' => (13, false),
        b'[' => (26, false),
        b']' => (27, false),
        b';' => (39, false),
        b'\'' => (40, false),
        b'`' => (41, false),
        b'\\' => (43, false),
        b',' => (51, false),
        b'.' => (52, false),
        b'/' => (53, false),
        b'!' => (2, true),
        b'@' => (3, true),
        b'#' => (4, true),
        b'$' => (5, true),
        b'%' => (6, true),
        b'^' => (7, true),
        b'&' => (8, true),
        b'*' => (9, true),
        b'(' => (10, true),
        b')' => (11, true),
        b'_' => (12, true),
        b'+' => (13, true),
        b'{' => (26, true),
        b'}' => (27, true),
        b':' => (39, true),
        b'"' => (40, true),
        b'~' => (41, true),
        b'|' => (43, true),
        b'<' => (51, true),
        b'>' => (52, true),
        b'?' => (53, true),
        _ => (0, false),
    };
    (code, shift, (ascii != 0).then_some(ascii))
}
