//! bwm's v2 compositor client. Chrome is an ordinary committed background window;
//! client content is composed by the kernel from immutable fronts above it.
use abi::{op, Plain};
use core::sync::atomic::{AtomicU64, Ordering};
pub use libbreenix::graphics::{
    cursor_shape, fbinfo, input_event_type, mouse_pos, mouse_state_with_scroll,
    poll_modifier_state, take_over_display, WindowInputEvent,
};
use libbreenix::{
    error::Error,
    window::{self as win, abi},
};
use std::sync::Mutex;
pub const COMPOSITOR_READY_DIRTY: u32 = 1;
pub const COMPOSITOR_READY_MOUSE: u32 = 2;
pub const COMPOSITOR_READY_REGISTRY: u32 = 4;
#[derive(Clone, Copy)]
pub struct WindowInfo {
    pub buffer_id: u64,
    pub owner_pid: u32,
    pub width: u32,
    pub height: u32,
    pub title_len: u32,
    pub title: [u8; 64],
}
static BACKGROUND: Mutex<Option<abi::Create>> = Mutex::new(None);
static EVENT_SERIAL: AtomicU64 = AtomicU64::new(0);
static CURSOR: Mutex<abi::Cursor> = Mutex::new(abi::Cursor {
    shape: 0,
    visible: 1,
    x: 0,
    y: 0,
});
static LAST: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());
pub fn init(w: u32, h: u32) -> Result<(), Error> {
    win::claim_compositor()?;
    let b = win::create(w, h, 2, 1)?;
    win::register(b.id, b"\x01BWM chrome")?;
    win::call(
        op::SET_GEOMETRY,
        &mut abi::Geometry {
            id: b.id,
            x: 0,
            y: 0,
            z: 0,
            flags: abi::geometry_flags::VISIBLE,
        },
    )?;
    *BACKGROUND.lock().unwrap() = Some(b);
    Ok(())
}
pub fn list_windows(out: &mut [WindowInfo]) -> Result<u64, Error> {
    let mut infos = [abi::WinInfo::ZERO; 17];
    win::list(&mut infos)?;
    let background = BACKGROUND.lock().unwrap().as_ref().map_or(0, |b| b.id);
    let mut n = 0;
    for i in infos
        .iter()
        .filter(|i| i.id != 0 && i.id != background)
        .take(out.len())
    {
        out[n] = WindowInfo {
            buffer_id: i.id,
            owner_pid: i.owner,
            width: i.width,
            height: i.height,
            title_len: i.title_len,
            title: i.title,
        };
        n += 1;
    }
    Ok(n as u64)
}
pub fn set_window_position(id: u64, x: i32, y: i32, z: u32, visible: bool) -> Result<(), Error> {
    win::call(
        op::SET_GEOMETRY,
        &mut abi::Geometry {
            id,
            x,
            y,
            z: z.saturating_add(1),
            flags: if visible {
                abi::geometry_flags::VISIBLE
            } else {
                abi::geometry_flags::MINIMIZED
            },
        },
    )
    .map(|_| ())
}
pub fn set_cursor_shape(shape: u32) -> Result<(), Error> {
    let mut c = CURSOR.lock().unwrap();
    c.shape = shape;
    win::call(op::SET_CURSOR, &mut *c).map(|_| ())
}
pub fn write_window_input(id: u64, old: &WindowInputEvent) -> Result<(), Error> {
    use abi::event_kind as kind;
    use input_event_type as old_kind;
    let k = match old.event_type {
        old_kind::KEY_PRESS => kind::KEY_PRESS,
        old_kind::KEY_RELEASE => kind::KEY_RELEASE,
        old_kind::MOUSE_MOVE => kind::POINTER_MOVE,
        old_kind::MOUSE_BUTTON => kind::POINTER_BUTTON,
        old_kind::MOUSE_SCROLL => kind::SCROLL,
        old_kind::FOCUS_GAINED => kind::FOCUS_GAINED,
        old_kind::FOCUS_LOST => kind::FOCUS_LOST,
        old_kind::CLOSE_REQUESTED => kind::CLOSE_REQUESTED,
        // The v2 protocol reserves Resized for the kernel's successful owner resize.
        // A compositor must not forge it with unallocated dimensions.
        _ => return Err(Error::Os(libbreenix::errno::Errno::EINVAL)),
    };
    let mut event = abi::WindowEvent::new(k);
    event.serial = EVENT_SERIAL.fetch_add(1, Ordering::Relaxed) + 1;
    event.time_ns = libbreenix::time::now_monotonic().map_or(0, |v| v.as_nanos() as u64);
    event.modifiers = old.modifiers as u32;
    event.keycode = old.keycode as u32;
    event.x = old.mouse_x as i32;
    event.y = old.mouse_y as i32;
    if k == kind::POINTER_BUTTON {
        event.button = abi::button::LEFT + old.keycode as u32;
        event.pressed = u32::from(old.scroll_y != 0);
    }
    if k == kind::SCROLL {
        event.scroll_y = old.scroll_y as i32;
    }
    let mut write = abi::WriteEvents {
        id,
        ptr: &event as *const _ as u64,
        count: 1,
        reserved: 0,
    };
    win::call(op::WRITE_EVENTS, &mut write).map(|_| ())
}
pub fn compose_scene(pixels: &[u32], w: u32, h: u32, changed: bool) -> Result<(), Error> {
    compose_region(pixels, w, h, u32::from(changed), 0, 0, w, h)
}
pub fn compose_region(
    pixels: &[u32],
    w: u32,
    h: u32,
    mode: u32,
    x: u32,
    y: u32,
    rw: u32,
    rh: u32,
) -> Result<(), Error> {
    if mode != 0 {
        let b = BACKGROUND.lock().unwrap();
        let b = b
            .as_ref()
            .ok_or(Error::Os(libbreenix::errno::Errno::EINVAL))?;
        if (w, h) != (b.width, b.height) || pixels.len() != (w * h) as usize {
            return Err(Error::Os(libbreenix::errno::Errno::EINVAL));
        }
        let a = win::acquire(b.id, 1_000_000_000)?;
        let r = if mode == 2 {
            abi::Rect::new(x as i32, y as i32, rw as i32, rh as i32)
        } else {
            abi::Rect::new(0, 0, w as i32, h as i32)
        };
        // The kernel replays unchanged rows on ACQUIRE. Copy only this chrome damage.
        for row in r.y.max(0) as u32..((r.y as i64 + r.h as i64).max(0) as u32).min(h) {
            let left = r.x.max(0) as u32;
            let right = ((r.x as i64 + r.w as i64).max(0) as u32).min(w);
            if right > left {
                let offset = (row * w + left) as usize;
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        pixels.as_ptr().add(offset),
                        (b.buffer[a.buffer as usize] as *mut u32).add(offset),
                        (right - left) as usize,
                    );
                }
            }
        }
        win::commit(b.id, a.buffer, &[r], None)?;
    }
    let (x, y) = mouse_pos()?;
    let mut c = CURSOR.lock().unwrap();
    c.x = x as i32;
    c.y = y as i32;
    win::call(op::SET_CURSOR, &mut *c)?;
    drop(c);
    win::composite().map(|_| ())
}
pub fn compositor_wait(_timeout: u32, generation: u32) -> Result<(u32, u32), Error> {
    // v2's LIST exposes commit ids; poll at a bounded UI cadence until the input-core
    // effort adds the compositor's combined registry/input event descriptor.
    libbreenix::time::sleep_ms(8)?;
    let mut infos = [abi::WinInfo::ZERO; 17];
    win::list(&mut infos)?;
    let current: Vec<_> = infos
        .iter()
        .filter(|i| i.id != 0)
        .map(|i| (i.id, i.commit_id))
        .collect();
    let mut last = LAST.lock().unwrap();
    let registry = last.iter().map(|v| v.0).ne(current.iter().map(|v| v.0));
    let changed = *last != current;
    *last = current;
    Ok((
        COMPOSITOR_READY_MOUSE
            | if changed { COMPOSITOR_READY_DIRTY } else { 0 }
            | if registry {
                COMPOSITOR_READY_REGISTRY
            } else {
                0
            },
        generation.wrapping_add(u32::from(registry)),
    ))
}
