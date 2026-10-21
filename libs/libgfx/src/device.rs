//! The process's BGL context and the static tables uploaded into it once per context
//! (docs/gui/breengel.md section 7, ruling B10).
//!
//! GPU handles are not inherited across `fork`, so a child that draws gets a context of its
//! own; the parent's handles are abandoned in the child without being destroyed there.

use bgl::Context;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};
use libbreenix::process;

pub(crate) struct Device {
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
    owner: AtomicU64,
    device: UnsafeCell<Option<Device>>,
}
// SAFETY: `device` is only reached while the PID-tagged owner lock is held.
unsafe impl Sync for Slot {}

static SLOT: Slot = Slot { owner: AtomicU64::new(0), device: UnsafeCell::new(None) };

/// The calling process's id, or 0 when it cannot be read.
pub(crate) fn pid() -> u64 {
    process::getpid().map(|p| p.raw()).unwrap_or(0)
}

/// Run `f` with this process's device, creating its context on first use (or first use
/// after `fork`). A context that the core no longer runs (EIO) is replaced on the next call.
pub(crate) fn with<R>(f: impl FnOnce(&mut Device) -> Result<R, i64>) -> Result<R, i64> {
    let me = pid();
    if me == 0 { return Err(-libbreenix::gpu::abi::errno::EIO); }
    let inherited = acquire(&SLOT.owner, me);
    struct Held<'a>(&'a AtomicU64, u64);
    impl Drop for Held<'_> {
        fn drop(&mut self) { self.0.store(self.1 << 1, Ordering::Release); }
    }
    let _held = Held(&SLOT.owner, me);
    // A fork may snapshot Device mid-mutation while another thread owns it. Abandon
    // those bytes without even reading or dropping the inherited Option's discriminant.
    if inherited {
        unsafe { SLOT.device.get().write(None); }
    }
    // SAFETY: the PID-tagged lock is held for the rest of this call.
    let slot = unsafe { &mut *SLOT.device.get() };
    if slot.is_none() {
        *slot = Some(Device { context: Context::new()?, tables: [None; TABLES] });
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

/// Keep the process id even while unlocked, so a child discards inherited state
/// before touching it. A lock owned by a different process cannot have a live owner here.
fn acquire(owner: &AtomicU64, pid: u64) -> bool {
    let mut seen = owner.load(Ordering::Relaxed);
    loop {
        let inherited = seen >> 1 != 0 && seen >> 1 != pid;
        if seen & 1 == 0 || inherited {
            match owner.compare_exchange_weak(seen, pid << 1 | 1, Ordering::Acquire, Ordering::Relaxed) {
                Ok(_) => return inherited,
                Err(next) => seen = next,
            }
        } else {
            core::hint::spin_loop();
            seen = owner.load(Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn fork_child_does_not_wait_for_the_parent_lock_owner() {
        unsafe extern "C" {
            fn getpid() -> i32;
            fn fork() -> i32;
            fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
            fn alarm(seconds: u32) -> u32;
            fn _exit(status: i32) -> !;
        }
        let parent = unsafe { getpid() } as u64;
        let owner = AtomicU64::new(parent << 1 | 1);
        let child = unsafe { fork() };
        assert!(child >= 0);
        if child == 0 {
            unsafe { alarm(2); }
            let inherited = acquire(&owner, unsafe { getpid() } as u64);
            unsafe { _exit(if inherited { 0 } else { 1 }); }
        }
        let mut status = -1;
        assert_eq!(unsafe { waitpid(child, &mut status, 0) }, child);
        assert_eq!(status, 0, "child failed or timed out on an inherited lock");
        assert_eq!(owner.load(Ordering::Relaxed), parent << 1 | 1);
    }
    #[test]
    fn child_takes_over_a_locked_parent_slot_before_accessing_device() {
        let owner = AtomicU64::new(41 << 1 | 1);
        assert!(acquire(&owner, 42));
        assert_eq!(owner.load(Ordering::Relaxed), 42 << 1 | 1);
        owner.store(42 << 1, Ordering::Release);
        assert!(!acquire(&owner, 42));
        owner.store(42 << 1, Ordering::Release);
        assert!(acquire(&owner, 43));
    }
}
