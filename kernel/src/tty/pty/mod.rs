//! PTY (Pseudo-Terminal) subsystem
//!
//! Provides pseudo-terminal support for remote shell access.

pub mod io;
pub mod ioctl;
pub mod pair;

// Re-export PtyPair for external use
pub use pair::PtyPair;

use crate::syscall::errno::{ENOMEM, ENOSPC};
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Mutex, Once};

/// Maximum number of PTY pairs
pub const MAX_PTYS: u32 = 256;

/// Global PTY allocator
static PTY_ALLOCATOR: Once<Mutex<PtyAllocator>> = Once::new();

struct PtyAllocator {
    next_pty_num: u32,
    /// Released PTY numbers available for reuse
    free_list: Vec<u32>,
    pairs: BTreeMap<u32, Arc<PtyPair>>,
}

impl PtyAllocator {
    fn new() -> Self {
        Self {
            next_pty_num: 0,
            free_list: Vec::new(),
            pairs: BTreeMap::new(),
        }
    }
}

/// Initialize the PTY subsystem
pub fn init() {
    PTY_ALLOCATOR.call_once(|| Mutex::new(PtyAllocator::new()));
    log::info!("PTY subsystem initialized");
}

/// Allocate a new PTY pair
pub fn allocate() -> Result<Arc<PtyPair>, i32> {
    let mut alloc = PTY_ALLOCATOR.get().ok_or(ENOMEM)?.lock();

    // Prefer recycling a released PTY number
    let pty_num = if let Some(recycled) = alloc.free_list.pop() {
        recycled
    } else if alloc.next_pty_num < MAX_PTYS {
        let num = alloc.next_pty_num;
        alloc.next_pty_num += 1;
        num
    } else {
        return Err(ENOSPC);
    };

    let pair = Arc::new(PtyPair::new(pty_num));
    alloc.pairs.insert(pty_num, pair.clone());

    Ok(pair)
}

/// Get an existing PTY pair by number
pub fn get(pty_num: u32) -> Option<Arc<PtyPair>> {
    PTY_ALLOCATOR.get()?.lock().pairs.get(&pty_num).cloned()
}

/// Release `pair`'s number for reuse, if the table still holds this pair.
fn release(pair: &Arc<PtyPair>) {
    if let Some(alloc) = PTY_ALLOCATOR.get() {
        let mut alloc = alloc.lock();
        if alloc
            .pairs
            .get(&pair.pty_num)
            .is_some_and(|current| Arc::ptr_eq(current, pair))
        {
            alloc.pairs.remove(&pair.pty_num);
            alloc.free_list.push(pair.pty_num);
        }
    }
}

/// Retire one master reference (a closed or discarded master fd).
///
/// Every master fd retire goes through here. The last reference marks the
/// master closed, releases the pair's number and hangs up the terminal.
pub fn close_master_ref(pair: &Arc<PtyPair>) {
    if pair.master_refcount.fetch_sub(1, core::sync::atomic::Ordering::SeqCst) != 1 {
        return;
    }
    pair.mark_master_closed();
    release(pair);
    hangup(pair.pty_num);
}

/// Retire one slave reference (a closed or discarded slave fd).
pub fn close_slave_ref(pair: &Arc<PtyPair>) {
    pair.slave_close();
}

/// A reference to one side of a pair held for the length of a read or a
/// write, as an open file is held by a system call using it: a close of the
/// descriptor meanwhile is not the last close, so it neither hangs the
/// terminal up nor reports the slave gone while the call is still using it.
pub struct PtyHold {
    pair: Arc<PtyPair>,
    master: bool,
}

impl PtyHold {
    /// Take a master reference. Called with the process manager held and a
    /// master descriptor of `pair` still in the caller's table, so the count
    /// is at least one and this is not a reopen of a closed master.
    pub fn master(pair: &Arc<PtyPair>) -> Self {
        pair.master_refcount.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        Self { pair: pair.clone(), master: true }
    }

    /// Take a slave reference, under the same conditions as `master`.
    pub fn slave(pair: &Arc<PtyPair>) -> Self {
        pair.slave_refcount.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        Self { pair: pair.clone(), master: false }
    }

    pub fn pair(&self) -> &Arc<PtyPair> {
        &self.pair
    }

    pub fn is_master(&self) -> bool {
        self.master
    }
}

impl Drop for PtyHold {
    fn drop(&mut self) {
        if self.master {
            close_master_ref(&self.pair);
        } else {
            close_slave_ref(&self.pair);
        }
    }
}

/// `ctty::hangup` takes the process manager, and some close paths hold it
/// (process teardown closes descriptors under it). Those hand the hangup to
/// the system workqueue.
fn hangup(pty_num: u32) {
    let id = crate::tty::ctty::TtyId::Pty(pty_num);
    if crate::process::process_manager_held_on_current_cpu() {
        crate::task::workqueue::schedule_work_fn(move || crate::tty::ctty::hangup(id), "pty_hangup");
    } else {
        crate::tty::ctty::hangup(id);
    }
}

/// Fallible snapshot for userspace directory and device operations.
pub fn try_list_active() -> Result<alloc::vec::Vec<u32>, u64> {
    let mut numbers = alloc::vec::Vec::new();
    if let Some(alloc) = PTY_ALLOCATOR.get() {
        let alloc = alloc.lock();
        numbers.try_reserve_exact(alloc.pairs.len()).map_err(|_| 12u64)?;
        numbers.extend(alloc.pairs.keys().copied());
    }
    Ok(numbers)
}

/// List all active (allocated) PTY numbers
/// Used by devpts filesystem for directory listing
pub fn list_active() -> alloc::vec::Vec<u32> {
    match PTY_ALLOCATOR.get() {
        Some(alloc) => alloc.lock().pairs.keys().copied().collect(),
        None => alloc::vec::Vec::new(),
    }
}
