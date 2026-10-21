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
 CompositeEntry, CompositeRecordHeader, Cursor, Plain, Readback,
    Rect,
};

use crate::gpu::window_targets::Target;
use super::{Caller, Damage, REGISTRY};

/// A pinned front buffer: its memory stays readable, and the buffer cannot return to its
/// owner, until the pin is dropped. Never drop a pin while holding the registry lock.
pub struct Pin {
    window: u64,
    uid: u64,
    mem: Arc<Target>,
}

impl Pin {
    /// Pin buffer `uid` of window `window`, whose memory is `mem`. The caller holds the registry
    /// lock and has incremented the buffer's pin count.
    pub(super) fn new(window: u64, uid: u64, mem: Arc<Target>) -> Pin {
        Pin { window, uid, mem }
    }

    /// The buffer's memory: BGRX rows of the window's stride.
    pub fn mem(&self) -> &Target {
        &self.mem
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        super::ops::unpin(self.window, self.uid);
    }
}

/// A composite the backend completed.
pub struct Composed {
    /// Pixels composed.
    pub pixels: u64,
    /// Pixels of each window composed, in `Frame::windows` order.
    pub per_window: Vec<u64>,
}

/// The display side of the window interface.
pub trait Backend: Sync {
    /// B13: validate and submit the caller's scene, retaining sampling pins in device holds.
    fn submit_scene(&self, _caller: &Caller, _frame: &mut super::scene::Admission, _arg: &mut breengel_abi::Composite) -> Result<Composed, i32> { Err(EIO) }
    /// This backend's `breengel_abi::backend` code.
    fn code(&self) -> u32;
    /// `SCANOUT_READBACK`, for root or the compositor; the backend writes the user
    /// destination and record, and sets `rb.serial`.
    fn readback(&self, _caller: &Caller, _rb: &mut Readback) -> Result<(), i32> {
        Err(EIO)
    }
    /// `SET_CURSOR`, for the compositor.
    fn set_cursor(&self, _caller: &Caller, _cursor: &Cursor) -> Result<(), i32> {
        Err(EIO)
    }
    /// `GIVE_DISPLAY`, for the display owner.
    fn give_display(&self, _caller: &Caller, _pid: u32) -> Result<(), i32> {
        Err(EIO)
    }
    /// The composite just completed is abandoned (its output could not be copied out): no
    /// observation may stand for it, and the next composite repairs the whole screen.
    fn abandon(&self) {}
}

static BACKEND: spin::Once<&'static dyn Backend> = spin::Once::new();

/// Register the display backend. The first registration wins.
pub fn set_backend(backend: &'static dyn Backend) {
    BACKEND.call_once(|| backend);
}

pub fn backend() -> Option<&'static dyn Backend> {
    BACKEND.get().copied()
}

/// A window's presentation state.
pub struct WindowPresent {
    /// Damage committed since the window was last composed, window coordinates.
    pub pending: Damage,
    /// Where the window was last composed, if it was on screen.
    pub shown: Option<Rect>,
    /// Its stacking position when it was last composed.
    pub shown_z: u32,
}

impl WindowPresent {
    pub fn new() -> WindowPresent {
        WindowPresent {
            pending: Damage::none(),
            shown: None,
            shown_z: 0,
        }
    }
}

/// A composite, completed or failed, kept for `COMPOSITE_RECORD`.
pub struct Record {
    pub serial: u64,
    pub pixels: u64,
    /// `breengel_abi::record_flags`.
    pub flags: u32,
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
pub(super) const RECORDS: usize = 64;

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
        let Some(r) = rect.filter(|r| !r.is_empty()) else {
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
    header.flags = r.flags;
    let mut entries = Vec::new();
    entries.try_reserve_exact(r.entries.len()).map_err(|_| EBUSY)?;
    entries.extend_from_slice(&r.entries);
    Ok((header, entries))
}
