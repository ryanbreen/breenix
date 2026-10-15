//! The compositor interface (shared piece 12): what a composite reads and how its result
//! comes back.
//!
//! The window layer keeps, for every window, the damage committed since it was last composed
//! and the rectangle it was last composed at, and for the screen the regions uncovered by
//! destroyed, moved or hidden windows. `COMPOSITE` turns that into a `Frame`: the screen damage
//! union since the previous composite (the whole screen the first time, and again after a
//! failure), and every presentation-required window bottom to top with its front pinned. The
//! display backend (`Backend`, registered by the display code) composes the frame; the window
//! layer records the result (`COMPOSITE_RECORD`), counts it and drops the pins when the call
//! returns, which returns retired buffers to their owners.

use alloc::{collections::VecDeque, sync::Arc, vec::Vec};

use breengel_abi::errno::{EBUSY, EIO, ENOENT, EPERM};
use breengel_abi::{
    backend, limits, CompositeEntry, CompositeRecordHeader, Cursor, MapTree, Plain, Readback,
    Rect,
};

use super::mem::Mem;
use super::{Caller, Damage, REGISTRY};

/// A pinned front buffer: its memory stays readable, and the buffer cannot return to its
/// owner, until the pin is dropped. Never drop a pin while holding the registry lock.
pub struct Pin {
    window: u64,
    uid: u64,
    mem: Arc<Mem>,
}

impl Pin {
    /// Pin buffer `uid` of window `window`, whose memory is `mem`. The caller holds the registry
    /// lock and has incremented the buffer's pin count.
    pub(super) fn new(window: u64, uid: u64, mem: Arc<Mem>) -> Pin {
        Pin { window, uid, mem }
    }

    /// The buffer's memory: BGRX rows of the window's stride.
    pub fn mem(&self) -> &Mem {
        &self.mem
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        super::ops::unpin(self.window, self.uid);
    }
}

/// One window of a frame.
pub struct FrameWindow {
    pub id: u64,
    /// Its position and size on the screen, unclipped.
    pub rect: Rect,
    /// Bytes per row of `pixels`.
    pub stride: u32,
    pub commit_id: u64,
    pub pixel_gen: u64,
    /// Its committed pixels, or `None` before its first commit (every pixel zero).
    pub pixels: Option<Pin>,
}

/// What one composite must produce.
pub struct Frame {
    pub serial: u64,
    /// Whether `damage` is the whole screen.
    pub full: bool,
    /// The screen pixels to compose, in screen coordinates, clipped to the screen.
    pub damage: Vec<Rect>,
    /// Every presentation-required window, bottom to top.
    pub windows: Vec<FrameWindow>,
}

/// A composite the backend completed.
pub struct Composed {
    /// Pixels composed.
    pub pixels: u64,
    /// Pixels of each window composed, in `Frame::windows` order.
    pub per_window: Vec<u64>,
}

/// A composite the backend could not complete, with the record's reason.
pub struct Failure {
    pub reason: u32,
}

/// The display side of the window interface.
pub trait Backend: Sync {
    /// This backend's `breengel_abi::backend` code.
    fn code(&self) -> u32;
    /// Compose `frame` onto the display.
    fn composite(&self, frame: &Frame) -> Result<Composed, Failure>;
    /// `SCANOUT_READBACK`, for root or the compositor; the backend writes the user
    /// destination and record, and sets `rb.serial`.
    fn readback(&self, _caller: &Caller, _rb: &mut Readback) -> Result<(), i32> {
        Err(EIO)
    }
    /// `SET_CURSOR`, for the compositor.
    fn set_cursor(&self, _caller: &Caller, _cursor: &Cursor) -> Result<(), i32> {
        Err(EIO)
    }
    /// `MAP_COMPOSITOR_TEXTURE`, for the compositor.
    fn map_texture(&self, _caller: &Caller, _map: &mut MapTree) -> Result<(), i32> {
        Err(EIO)
    }
    /// `GIVE_DISPLAY`, for the display owner.
    fn give_display(&self, _caller: &Caller, _pid: u32) -> Result<(), i32> {
        Err(EIO)
    }
}

static BACKEND: spin::Once<&'static dyn Backend> = spin::Once::new();

/// Register the display backend. The first registration wins.
pub fn set_backend(backend: &'static dyn Backend) {
    BACKEND.call_once(|| backend);
}

pub fn backend() -> Option<&'static dyn Backend> {
    BACKEND.get().copied()
}

/// The code `WinInfo.backend` reports.
pub fn backend_code() -> u32 {
    backend().map_or(backend::NONE, |b| b.code())
}

/// A window's presentation state.
pub struct WindowPresent {
    /// Damage committed since the window was last composed, window coordinates.
    pub pending: Damage,
    /// Where the window was last composed, if it was on screen.
    pub shown: Option<Rect>,
}

impl WindowPresent {
    pub fn new() -> WindowPresent {
        WindowPresent {
            pending: Damage::none(),
            shown: None,
        }
    }
}

/// A completed composite, kept for `COMPOSITE_RECORD`.
pub struct Record {
    pub serial: u64,
    pub pixels: u64,
    pub entries: Vec<CompositeEntry>,
}

/// The screen's presentation state.
pub struct State {
    pub serial: u64,
    /// Whether a composite has succeeded since boot (the first composes the whole screen).
    pub composed_once: bool,
    /// Whether the next composite must compose the whole screen (after a failure).
    pub force_full: bool,
    /// Screen regions uncovered by windows destroyed, resized, moved or hidden.
    pub uncovered: Vec<Rect>,
    /// Whether a composite is running.
    pub busy: bool,
    pub records: VecDeque<Record>,
}

/// Records kept for `COMPOSITE_RECORD`.
const RECORDS: usize = 64;

impl State {
    pub const fn new() -> State {
        State {
            serial: 0,
            composed_once: false,
            force_full: false,
            uncovered: Vec::new(),
            busy: false,
            records: VecDeque::new(),
        }
    }

    /// Note that `rect` of the screen no longer shows what it did.
    pub fn uncover(&mut self, rect: Option<Rect>) {
        let Some(r) = rect.and_then(clip_to_screen) else {
            return;
        };
        if self.uncovered.try_reserve(1).is_err() || self.uncovered.len() >= 64 {
            self.force_full = true;
            self.uncovered.clear();
            return;
        }
        self.uncovered.push(r);
    }
}

fn screen() -> Rect {
    Rect::new(0, 0, limits::SCREEN_WIDTH as i32, limits::SCREEN_HEIGHT as i32)
}

/// `r` clipped to the screen, or `None` when nothing of it is on screen.
pub fn clip_to_screen(r: Rect) -> Option<Rect> {
    intersect(r, screen())
}

/// The intersection of two rectangles, or `None` when it is empty.
pub fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let x0 = (a.x as i64).max(b.x as i64);
    let y0 = (a.y as i64).max(b.y as i64);
    let x1 = (a.x as i64 + a.w.max(0) as i64).min(b.x as i64 + b.w.max(0) as i64);
    let y1 = (a.y as i64 + a.h.max(0) as i64).min(b.y as i64 + b.h.max(0) as i64);
    (x1 > x0 && y1 > y0).then(|| Rect::new(x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32))
}

/// `COMPOSITE` (compositor only): compose the damage since the previous composite.
pub fn composite(caller: &Caller) -> Result<(u64, u64), i32> {
    let backend = backend();
    let frame = {
        let mut reg = REGISTRY.lock();
        if !reg.is_compositor(caller) {
            return Err(EPERM);
        }
        if reg.present.busy {
            return Err(EBUSY);
        }
        let Some(_) = backend else {
            return Err(EIO);
        };
        begin(&mut reg)?
    };
    let result = backend.map_or(Err(Failure { reason: 0 }), |b| b.composite(&frame));
    finish(frame, result)
}

/// Build the next frame, under the registry lock: assign its serial, collect its damage and
/// pin every presentation-required window's front.
fn begin(reg: &mut super::Registry) -> Result<Frame, i32> {
    let mut damage = Vec::new();
    let full = !reg.present.composed_once || reg.present.force_full;
    if full {
        damage.push(screen());
    } else {
        for r in reg.present.uncovered.iter() {
            push_rect(&mut damage, *r);
        }
        for w in reg.windows.values() {
            let now = w.screen_rect();
            if w.present.shown != now {
                if let Some(r) = w.present.shown {
                    push_rect(&mut damage, r);
                }
                if let Some(r) = now {
                    push_rect(&mut damage, r);
                }
            } else if let Some(at) = now {
                for r in w.present.pending.rects(w.width, w.height) {
                    push_rect(&mut damage, Rect::new(at.x + r.x, at.y + r.y, r.w, r.h));
                }
            }
        }
        if damage.len() > 64 {
            let b = super::bounds(&damage);
            damage.clear();
            damage.push(b);
        }
    }
    let mut order: Vec<(u32, u64)> = reg
        .windows
        .values()
        .filter(|w| w.presentation_required())
        .map(|w| (w.z, w.id))
        .collect();
    order.sort_unstable();
    let mut windows = Vec::new();
    windows.try_reserve_exact(order.len()).map_err(|_| EBUSY)?;
    for (_, id) in order {
        let Some(w) = reg.windows.get_mut(&id) else {
            continue;
        };
        let pixels = match w.front {
            Some(f) => {
                let b = &mut w.buffers[f];
                b.pins += 1;
                Some(Pin::new(id, b.uid, b.mem.clone()))
            }
            None => None,
        };
        windows.push(FrameWindow {
            id,
            rect: Rect::new(w.x, w.y, w.width as i32, w.height as i32),
            stride: w.stride(),
            commit_id: w.commit_id,
            pixel_gen: w.pixel_gen,
            pixels,
        });
    }
    for w in reg.windows.values_mut() {
        w.present.pending = Damage::none();
        w.present.shown = w.screen_rect();
    }
    reg.present.uncovered.clear();
    reg.present.force_full = false;
    reg.present.serial += 1;
    reg.present.busy = true;
    Ok(Frame {
        serial: reg.present.serial,
        full,
        damage,
        windows,
    })
}

fn push_rect(damage: &mut Vec<Rect>, r: Rect) {
    if let Some(r) = clip_to_screen(r) {
        if damage.try_reserve(1).is_ok() {
            damage.push(r);
        }
    }
}

/// Record a composite's result and release its pins. A failed composite leaves its damage to
/// the next one, which composes the whole screen.
fn finish(frame: Frame, result: Result<Composed, Failure>) -> Result<(u64, u64), i32> {
    let outcome = {
        let mut reg = REGISTRY.lock();
        reg.present.busy = false;
        match &result {
            Ok(done) => {
                reg.present.composed_once = true;
                let mut entries = Vec::new();
                if entries.try_reserve_exact(frame.windows.len()).is_ok() {
                    for (i, w) in frame.windows.iter().enumerate() {
                        entries.push(CompositeEntry {
                            id: w.id,
                            commit_id: w.commit_id,
                            pixel_gen: w.pixel_gen,
                            pixels: done.per_window.get(i).copied().unwrap_or(0),
                        });
                    }
                }
                if reg.present.records.len() == RECORDS {
                    reg.present.records.pop_front();
                }
                if reg.present.records.try_reserve(1).is_ok() {
                    reg.present.records.push_back(Record {
                        serial: frame.serial,
                        pixels: done.pixels,
                        entries,
                    });
                }
                Ok((frame.serial, done.pixels))
            }
            Err(_) => {
                reg.present.force_full = true;
                Err(EIO)
            }
        }
    };
    if let Ok(done) = &result {
        crate::tracing::providers::window::count_composite(done.pixels);
    }
    drop(frame);
    outcome
}

/// `COMPOSITE_RECORD`: the record of composite `serial`, as its header and entries.
pub fn record(caller: &Caller, serial: u64) -> Result<(CompositeRecordHeader, Vec<CompositeEntry>), i32> {
    let reg = REGISTRY.lock();
    if !reg.is_compositor(caller) {
        return Err(EPERM);
    }
    let r = reg
        .present
        .records
        .iter()
        .find(|r| r.serial == serial)
        .ok_or(ENOENT)?;
    let mut header = CompositeRecordHeader::ZERO;
    header.serial = r.serial;
    header.pixels = r.pixels;
    header.count = r.entries.len() as u32;
    let mut entries = Vec::new();
    entries.try_reserve_exact(r.entries.len()).map_err(|_| EBUSY)?;
    entries.extend_from_slice(&r.entries);
    Ok((header, entries))
}
