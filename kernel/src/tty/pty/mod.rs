//! PTY (Pseudo-Terminal) subsystem
//!
//! Provides pseudo-terminal support for remote shell access.

pub mod io;
pub mod ioctl;
pub mod pair;

// Re-export PtyPair for external use
pub use pair::PtyPair;

use crate::syscall::errno::{EIO, ENOENT, ENOMEM, ENOSPC, ESRCH};
use crate::syscall::SyscallResult;
use crate::task::completion::Completion;
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};
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

/// Open a new master: posix_openpt(3) and open("/dev/ptmx"). `flags` are
/// open(2) flags; O_CLOEXEC and O_NONBLOCK are kept on the descriptor.
pub fn open_master(flags: u32) -> SyscallResult {
    use crate::ipc::fd::{FdKind, FileDescriptor};

    // A master's last close may need the hangup thread; make no master
    // without it.
    if let Err(e) = ensure_hangup_daemon() {
        return SyscallResult::Err(e as u64);
    }
    let Some(thread_id) = crate::task::scheduler::current_thread_id() else {
        return SyscallResult::Err(ESRCH as u64);
    };
    let mut manager_guard = crate::process::manager();
    let Some((_, process)) = manager_guard
        .as_mut()
        .and_then(|manager| manager.find_process_by_thread_mut(thread_id))
    else {
        return SyscallResult::Err(ESRCH as u64);
    };
    let pair = match allocate() {
        Ok(pair) => pair,
        Err(e) => return SyscallResult::Err(e as u64),
    };
    let installed = FileDescriptor::try_opened(FdKind::PtyMaster(pair.clone()), flags)
        .and_then(|entry| process.fd_table.alloc_with_entry(entry).map_err(|e| e as u64));
    drop(manager_guard);
    match installed {
        Ok(fd) => SyscallResult::Ok(fd as u64),
        Err(e) => {
            close_master_ref(&pair);
            SyscallResult::Err(e)
        }
    }
}

/// Retire one master reference (a closed or discarded master fd).
///
/// Every master fd retire goes through here. The last reference marks the
/// master closed and hangs up the terminal; the pair keeps its number until
/// the hangup has finished, and is released once the slave side is gone too.
pub fn close_master_ref(pair: &Arc<PtyPair>) {
    // The last reference's decrement, the hangup reservation and the closed
    // mark are one step under the table lock, which `release_if_unused` holds
    // while it decides: it never sees the master gone without the
    // reservation, and an open of the slave never sees the master count at
    // zero without the closed mark.
    let last = {
        let Some(alloc) = PTY_ALLOCATOR.get() else {
            return;
        };
        let _table = alloc.lock();
        let last = pair.master_refcount.fetch_sub(1, Ordering::SeqCst) == 1;
        if last {
            pair.hangup_pending.store(true, Ordering::SeqCst);
            pair.mark_master_closed();
        }
        last
    };
    if !last {
        return;
    }
    pair.wake_slave_side();
    // `ctty::hangup` takes the process manager, and teardown closes a dying
    // process's descriptors with it held. Those closes leave the hangup to
    // the hangup thread, which they wake without taking a lock.
    if crate::process::process_manager_held_on_current_cpu() {
        if HANGUP_DAEMON_STATE.load(Ordering::Acquire) == DAEMON_RUNNING {
            PENDING_HANGUPS[pair.pty_num as usize / 64]
                .fetch_or(1 << (pair.pty_num % 64), Ordering::SeqCst);
            HANGUP_KICK.complete(1);
        } else {
            // Only a pair the kernel allocated for itself, never a master
            // descriptor (open_master refuses without the thread), gets here.
            pair.hangup_pending.store(false, Ordering::SeqCst);
            release_if_unused(pair);
        }
    } else {
        finish_hangup(pair);
    }
}

/// Retire one slave reference (a closed or discarded slave fd).
pub fn close_slave_ref(pair: &Arc<PtyPair>) {
    if pair.slave_close() {
        release_if_unused(pair);
    }
}

/// Run the last master close's hangup, then let the pair go.
fn finish_hangup(pair: &Arc<PtyPair>) {
    crate::tty::ctty::hangup(crate::tty::ctty::TtyId::Pty(pair.pty_num));
    pair.hangup_pending.store(false, Ordering::SeqCst);
    release_if_unused(pair);
}

/// Release `pair`'s number once no master and no slave reference remains
/// and no hangup is pending, so a hangup always finds its own pair.
fn release_if_unused(pair: &Arc<PtyPair>) {
    let Some(alloc) = PTY_ALLOCATOR.get() else {
        return;
    };
    let mut alloc = alloc.lock();
    if pair.master_refcount.load(Ordering::SeqCst) != 0
        || pair.slave_refcount.load(Ordering::SeqCst) != 0
        || pair.hangup_pending.load(Ordering::SeqCst)
    {
        return;
    }
    if alloc
        .pairs
        .get(&pair.pty_num)
        .is_some_and(|current| Arc::ptr_eq(current, pair))
    {
        alloc.pairs.remove(&pair.pty_num);
        alloc.free_list.push(pair.pty_num);
    }
}

/// Take a new slave reference on pair `pty_num` for an open of its slave.
///
/// Fails with EIO while the pair is locked or once its master has closed,
/// and with ENOENT when no such pair exists.
pub fn open_slave(pty_num: u32) -> Result<Arc<PtyPair>, i32> {
    let alloc = PTY_ALLOCATOR.get().ok_or(ENOENT)?;
    let table = alloc.lock();
    let pair = table.pairs.get(&pty_num).cloned().ok_or(ENOENT)?;
    // Under the table lock the closed mark and the master count agree, and
    // the pair cannot be released, so the reference taken here is either on
    // a live master's pair or not taken at all.
    if !pair.is_unlocked() || pair.master_closed() {
        return Err(EIO);
    }
    pair.slave_open();
    drop(table);
    Ok(pair)
}

/// Last master closes made with the process manager held, one bit per pair.
static PENDING_HANGUPS: [AtomicU64; (MAX_PTYS as usize).div_ceil(64)] =
    [const { AtomicU64::new(0) }; (MAX_PTYS as usize).div_ceil(64)];

/// Wakes the hangup thread; completed lock-free from any context.
static HANGUP_KICK: Completion = Completion::new();

const DAEMON_STOPPED: u8 = 0;
const DAEMON_STARTING: u8 = 1;
const DAEMON_RUNNING: u8 = 2;
static HANGUP_DAEMON_STATE: AtomicU8 = AtomicU8::new(DAEMON_STOPPED);

/// How long the idle hangup thread sleeps before looking again unprompted.
const HANGUP_DAEMON_IDLE_NS: u64 = 10_000_000_000;

/// Make sure the hangup thread runs, starting it if need be. Called with no
/// locks held, before a master descriptor is made. A start that fails is
/// tried again by the next caller; until one succeeds, no master is made
/// (`open_master` fails with ENOMEM), so no last close can need the thread
/// and find it missing.
pub fn ensure_hangup_daemon() -> Result<(), i32> {
    loop {
        match HANGUP_DAEMON_STATE.compare_exchange(
            DAEMON_STOPPED,
            DAEMON_STARTING,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                return match crate::task::kthread::kthread_run(hangup_daemon, "kptyhup") {
                    Ok(_) => {
                        HANGUP_DAEMON_STATE.store(DAEMON_RUNNING, Ordering::Release);
                        Ok(())
                    }
                    Err(_) => {
                        HANGUP_DAEMON_STATE.store(DAEMON_STOPPED, Ordering::Release);
                        Err(ENOMEM)
                    }
                };
            }
            Err(DAEMON_RUNNING) => return Ok(()),
            // Another caller is starting it.
            Err(_) => crate::task::scheduler::yield_current(),
        }
    }
}

/// Runs the hangups `close_master_ref` could not run itself.
fn hangup_daemon() {
    loop {
        HANGUP_KICK.reset();
        for (word, pending) in PENDING_HANGUPS.iter().enumerate() {
            let mut bits = pending.swap(0, Ordering::SeqCst);
            while bits != 0 {
                let pty_num = (word * 64) as u32 + bits.trailing_zeros();
                bits &= bits - 1;
                if let Some(pair) = get(pty_num) {
                    finish_hangup(&pair);
                }
            }
        }
        if PENDING_HANGUPS.iter().any(|pending| pending.load(Ordering::SeqCst) != 0) {
            continue;
        }
        // The brake selects the completion's sleeping wait, as in a syscall.
        crate::per_cpu::preempt_disable();
        let _ = HANGUP_KICK.wait_timeout_uninterruptible(1, HANGUP_DAEMON_IDLE_NS);
        crate::per_cpu::preempt_enable();
    }
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
        pair.master_refcount.fetch_add(1, Ordering::SeqCst);
        Self { pair: pair.clone(), master: true }
    }

    /// Take a slave reference, under the same conditions as `master`.
    pub fn slave(pair: &Arc<PtyPair>) -> Self {
        pair.slave_refcount.fetch_add(1, Ordering::SeqCst);
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
