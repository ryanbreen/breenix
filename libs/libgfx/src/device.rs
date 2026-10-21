//! The process's BGL context and the static tables uploaded into it once per context
//! (docs/gui/breengel.md section 7, ruling B10).
//!
//! GPU handles are not inherited across `fork`, so a child that draws gets a context of its
//! own; the parent's handles are abandoned in the child without being destroyed there.

use bgl::Context;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};
use libbreenix::process;

pub(crate) struct Device {
    pid: u64,
    pub(crate) context: Context,
    /// Textures holding a declared table, by `Table`.
    tables: [Option<Table>; TABLES],
}

/// A static table's texture and its glyph cell size.
#[derive(Clone, Copy)]
pub(crate) struct Table {
    pub(crate) texture: u64,
    pub(crate) cell_w: u32,
    pub(crate) cell_h: u32,
}

pub(crate) const TABLES: usize = 3;

struct Slot {
    busy: AtomicBool,
    device: UnsafeCell<Option<Device>>,
}
// SAFETY: `device` is only reached while `busy` is held.
unsafe impl Sync for Slot {}

static SLOT: Slot = Slot { busy: AtomicBool::new(false), device: UnsafeCell::new(None) };

/// The calling process's id, or 0 when it cannot be read.
pub(crate) fn pid() -> u64 {
    process::getpid().map(|p| p.raw()).unwrap_or(0)
}

/// Run `f` with this process's device, creating its context on first use (or first use
/// after `fork`). A context that the core no longer runs (EIO) is replaced on the next call.
pub(crate) fn with<R>(f: impl FnOnce(&mut Device) -> Result<R, i64>) -> Result<R, i64> {
    while SLOT.busy.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    struct Held<'a>(&'a AtomicBool);
    impl Drop for Held<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let _held = Held(&SLOT.busy);
    // SAFETY: `busy` is held for the rest of this call.
    let slot = unsafe { &mut *SLOT.device.get() };
    let me = pid();
    if slot.as_ref().is_some_and(|d| d.pid != me) {
        // The parent's context and textures are not this process's handles.
        core::mem::forget(slot.take());
    }
    if slot.is_none() {
        *slot = Some(Device { pid: me, context: Context::new()?, tables: [None; TABLES] });
    }
    let device = slot.as_mut().expect("device created above");
    let result = f(device);
    if result.as_ref().err() == Some(&-libbreenix::gpu::abi::errno::EIO) {
        *slot = None;
    }
    result
}

impl Device {
    /// The texture of table `index`, made by `make` (which uploads it) the first time.
    pub(crate) fn table(
        &mut self,
        index: usize,
        make: impl FnOnce(&Context) -> Result<Table, i64>,
    ) -> Result<Table, i64> {
        if let Some(t) = self.tables[index] {
            return Ok(t);
        }
        let t = make(&self.context)?;
        self.tables[index] = Some(t);
        Ok(t)
    }
}
