//! bwm's side of the window interface, version 2 (`docs/gui/breengel.md` sections 17 and
//! 19): the compositor claim, the registered windows, their geometry, the events bwm writes
//! to them, the scene descriptor it sleeps on, and the input it reads itself (the pointer and
//! the pollable input stream).


use libbreenix::error::Error;
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
    /// Physical core key code; text is present only on printable presses.
    Key { keycode: u32, modifiers: u32, repeat: bool, text: Option<u8> },
    KeyRelease { keycode: u32, modifiers: u32 },
    Text { bytes: [u8; 240], len: u32 },
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

/// Encode a policy notice or a core input event before source stamping.
pub fn event(input: Input) -> abi::WindowEvent {
    use abi::event_kind as kind;
    let mut event = abi::WindowEvent::ZERO;
    match input {
        Input::Key { keycode, modifiers, repeat, text } => {
            event.kind = kind::KEY_PRESS;
            event.modifiers = modifiers;
            event.keycode = keycode;
            if let Some(t) = text {
                event.text[0] = t;
                event.text_len = 1;
            }
            if repeat { event.flags |= abi::EVENT_REPEAT; }
        }
        Input::KeyRelease { keycode, modifiers } => {
            event.kind = kind::KEY_RELEASE;
            event.keycode = keycode;
            event.modifiers = modifiers;
        }
        Input::Text { bytes, len } => {
            event.kind = kind::TEXT;
            event.text = bytes;
            event.text_len = len;
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
                _ => 0,
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
    event
}

/// Submit an already stamped source or derived event without translation.
pub fn send(id: u64, event: abi::WindowEvent) -> Result<usize, Error> {
    window::write_events(id, &[event])
}
