//! The process's BGL context and the static tables uploaded into it once per context
//! (docs/gui/breengel.md section 7, ruling B10).
//!
//! GPU handles are not inherited across `fork`, so a child that draws gets a context of its
//! own; the parent's handles are abandoned in the child without being destroyed there.
//!
//! Shapes on a surface that defers them (`Surface::defer_shapes`) wait here, after one
//! `BEGIN` of their target, until anything else uses the device, or until their outcome is
//! asked for; they are then sent together, in order.

use alloc::vec::Vec;
use bgl::Context;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(target_os = "breenix")]
use libbreenix::process;

pub(crate) struct Device {
    pub(crate) context: Context,
    /// Textures holding a declared table, by `Table`.
    tables: [Option<Table>; TABLES],
    /// The deferred shapes of one target, not yet sent.
    deferred: Option<Deferred>,
    /// What sending deferred shapes gave each deferring target, until it is dropped.
    outcomes: Vec<Outcome>,
}

struct Deferred { target: u64, width: u32, height: u32, segments: Vec<Vec<u32>> }

struct Outcome { target: u64, error: Option<i64>, fence: Option<bgl::Fence> }

/// The most deferred shapes one submission carries, far below SUBMIT's draw cap.
const DEFERRED_SHAPES: usize = 256;

/// Whether shapes are deferred, so that a reader that only checks need not lock.
static PENDING: AtomicBool = AtomicBool::new(false);

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
    #[cfg(target_os = "breenix")]
    { process::getpid().map(|p| p.raw()).unwrap_or(0) }
    #[cfg(not(target_os = "breenix"))]
    { unsafe extern "C" { fn getpid() -> i32; } unsafe { getpid() as u64 } }
}

/// Run `f` with this process's device, creating its context on first use (or first use
/// after `fork`). A context that the core no longer runs (EIO) is replaced on the next call.
/// Deferred shapes are sent before `f` runs, so its work follows them.
pub(crate) fn with<R>(f: impl FnOnce(&mut Device) -> Result<R, i64>) -> Result<R, i64> {
    locked(|slot| {
        let device = created(slot)?;
        device.flush();
        let result = f(device);
        if result.as_ref().err() == Some(&-libbreenix::gpu::abi::errno::EIO) {
            *slot = None;
        }
        result
    })
}

/// Keep `encode`'s commands for `target` (`width × height`) with the deferred shapes, after
/// sending the shapes of another target first.
pub(crate) fn defer(target: u64, width: u32, height: u32, encode: impl FnOnce(&mut Context)) -> Result<(), i64> {
    locked(|slot| {
        let device = created(slot)?;
        if device.deferred.as_ref().is_some_and(|q| q.target != target) {
            device.flush();
        }
        let context = &mut device.context;
        context.begin(target, width, height);
        let start = context.encoded().len();
        encode(context);
        let words = context.encoded()[start..].to_vec();
        context.discard();
        let q = device.deferred.get_or_insert_with(|| Deferred { target, width, height, segments: Vec::new() });
        q.segments.push(words);
        PENDING.store(true, Ordering::Relaxed);
        if q.segments.len() >= DEFERRED_SHAPES {
            device.flush();
        }
        Ok(())
    })
}

/// Send any deferred shapes, without creating a device that does not exist.
pub(crate) fn flush() {
    if PENDING.load(Ordering::Relaxed) {
        let _ = locked(|slot| { if let Some(d) = slot.as_mut() { d.flush(); } Ok(()) });
    }
}

/// Send any deferred shapes and report what sending `target`'s gave: the first error and
/// the fence of the last submission.
pub(crate) fn settle(target: u64) -> (Option<i64>, Option<bgl::Fence>) {
    locked(|slot| Ok(slot.as_mut().map_or((None, None), |d| {
        d.flush();
        d.outcomes.iter().find(|o| o.target == target).map_or((None, None), |o| (o.error, o.fence))
    }))).unwrap_or((None, None))
}

/// Discard a dropped target's deferred shapes and outcome.
pub(crate) fn forget(target: u64) {
    let _ = locked(|slot| {
        if let Some(d) = slot.as_mut() {
            if d.deferred.as_ref().is_some_and(|q| q.target == target) {
                d.deferred = None;
                PENDING.store(false, Ordering::Relaxed);
            }
            d.outcomes.retain(|o| o.target != target);
        }
        Ok(())
    });
}

fn created(slot: &mut Option<Device>) -> Result<&mut Device, i64> {
    if slot.is_none() {
        *slot = Some(Device { context: Context::new()?, tables: [None; TABLES], deferred: None, outcomes: Vec::new() });
    }
    Ok(slot.as_mut().expect("device created above"))
}

/// Run `f` with the process's device slot under the PID-tagged lock.
fn locked<R>(f: impl FnOnce(&mut Option<Device>) -> Result<R, i64>) -> Result<R, i64> {
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
        PENDING.store(false, Ordering::Relaxed);
    }
    // SAFETY: the PID-tagged lock is held for the rest of this call.
    f(unsafe { &mut *SLOT.device.get() })
}

impl Device {
    /// Send the deferred shapes in one submission. One the core refuses is sent again in
    /// halves, down to a single shape, which fails as it would have alone.
    fn flush(&mut self) {
        let Some(q) = self.deferred.take() else { return };
        PENDING.store(false, Ordering::Relaxed);
        let i = match self.outcomes.iter().position(|o| o.target == q.target) {
            Some(i) => i,
            None => { self.outcomes.push(Outcome { target: q.target, error: None, fence: None }); self.outcomes.len() - 1 }
        };
        self.send(&q, &q.segments, i);
    }

    fn send(&mut self, q: &Deferred, segments: &[Vec<u32>], outcome: usize) {
        if segments.is_empty() { return; }
        self.context.begin(q.target, q.width, q.height);
        for words in segments { self.context.append(words); }
        let result = self.context.submit().inspect_err(|_| self.context.discard());
        crate::surface::count_submission();
        match result {
            Ok(fence) => self.outcomes[outcome].fence = Some(fence),
            Err(e) if segments.len() == 1 => { self.outcomes[outcome].error.get_or_insert(e); }
            Err(_) => {
                let (first, second) = segments.split_at(segments.len() / 2);
                self.send(q, first, outcome);
                self.send(q, second, outcome);
            }
        }
    }

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
