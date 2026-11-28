//! Mount exclusion that sleeps when contended (E24 coordinator ledger 309, C1).
//!
//! A mount's holder reads nodes and data through the buffer cache and sleeps
//! on block I/O with the exclusion held. Under a spin lock its contenders keep
//! their CPUs with preemption disabled (x86 syscalls also mask interrupts)
//! while the holder waits for one to finish on: ext2's #728. Contenders here
//! park on a wait queue instead, as ext2's #728 park path does
//! (fs/ext2/mod.rs, ext2_acquire), and spin only where the context cannot
//! sleep. Holding the exclusion changes no preemption count, so a holder's
//! block waits keep the single preemption brake they assert.
use crate::{fs::errno::Errno, syscall::errno::EINTR};
use core::{
    cell::UnsafeCell,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

/// One park round. A missed wake becomes a retry at this granularity rather
/// than a hang, as ext2's #728 park bounds its rounds.
#[cfg(not(test))]
const PARK_ROUND_NS: u64 = 200_000_000;

pub struct MountLock<T> {
    held: AtomicBool,
    // Contenders between announcing a park and finishing it. Release wakes
    // only when this is nonzero. The release's store-then-load and the
    // contender's increment-then-recheck are the store-buffering shape, so
    // each side has a full barrier between them (`task::park::announce` and
    // `should_wake`, design §3.4): either the release sees the contender or
    // the contender's recheck sees the lock free.
    parked: AtomicU32,
    #[cfg(not(test))]
    queue: crate::task::waitqueue::WaitQueueHead,
    #[cfg(test)]
    host: HostPark,
    value: UnsafeCell<T>,
}

// The exclusion hands out one guard at a time, as spin::Mutex does.
unsafe impl<T: Send> Send for MountLock<T> {}
unsafe impl<T: Send> Sync for MountLock<T> {}

#[must_use]
pub struct MountGuard<'a, T> {
    lock: &'a MountLock<T>,
    #[cfg(not(test))]
    io_custody: Option<crate::task::thread::KillCustody>,
}

impl<T> MountGuard<'_, T> {
    /// An admitted edit can need another metadata read after its first change.
    /// Let the edit finish before a pending kill unwinds the syscall.
    pub(super) fn protect_edit(&mut self) -> Result<(), Errno> {
        #[cfg(not(test))]
        {
            if crate::task::thread::current_wait_must_unwind() {
                return Err(Errno(EINTR));
            }
            self.io_custody = Some(crate::task::thread::KillCustody::try_enter().ok_or(Errno(EINTR))?);
        }
        Ok(())
    }
}

impl<T> MountLock<T> {
    pub fn new(value: T) -> Self {
        Self {
            held: AtomicBool::new(false),
            parked: AtomicU32::new(0),
            #[cfg(not(test))]
            queue: crate::task::waitqueue::WaitQueueHead::new(),
            #[cfg(test)]
            host: HostPark::default(),
            value: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> MountGuard<'_, T> {
        loop {
            if let Some(guard) = self.try_lock() {
                return guard;
            }
            self.park(false);
        }
    }

    /// Use only before taking on work that needs to reacquire this lock to finish.
    pub fn lock_interruptible(&self) -> Result<MountGuard<'_, T>, Errno> {
        loop {
            #[cfg(not(test))]
            if crate::task::thread::current_wait_must_unwind() {
                return Err(Errno(EINTR));
            }
            if let Some(guard) = self.try_lock() {
                return Ok(guard);
            }
            if !self.park(true) {
                return Err(Errno(EINTR));
            }
        }
    }

    pub fn try_lock(&self) -> Option<MountGuard<'_, T>> {
        self.held
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
            .ok()
            .map(|_| {
                #[cfg(test)]
                HELD.with(|held| held.set(held.get() + 1));
                #[cfg(test)]
                super::preempt::context::observe(super::preempt::context::Event::Acquire(self as *const Self as usize));
                MountGuard {
                    lock: self,
                    #[cfg(not(test))]
                    io_custody: None,
                }
            })
    }

    // Release first, then wake: no mount state is held across the wake.
    fn release(&self) {
        #[cfg(test)]
        super::preempt::context::observe(super::preempt::context::Event::Release(self as *const Self as usize));
        #[cfg(test)]
        HELD.with(|held| held.set(held.get() - 1));
        self.held.store(false, Ordering::SeqCst);
        #[cfg(not(test))]
        let parked = crate::task::park::should_wake(&self.parked);
        #[cfg(test)]
        let parked = self.parked.load(Ordering::SeqCst) != 0;
        if parked {
            self.wake();
        }
    }

    /// Park until the lock is released or a round ends. False when an
    /// interruptible acquisition must unwind for a kill. A context that may
    /// not park (`task::park::context`) spins instead, with a receipt. Since
    /// E2.4 the boot continuation reads no storage, so it never waits here.
    #[cfg(not(test))]
    fn park(&self, interruptible: bool) -> bool {
        use crate::task::{
            park::{self, WaitEnd, WaitMode},
            thread::current_wait_must_unwind,
        };
        park::announce(&self.parked);
        let deadline = super::commit_signal::monotonic_ns().saturating_add(PARK_ROUND_NS);
        // The recheck runs under the queue lock, atomically with publishing
        // the wait, so a release before the enqueue is seen here.
        let end = park::wait(
            &self.queue,
            || !self.held.load(Ordering::SeqCst),
            Some(deadline),
            WaitMode::Uninterruptible,
        );
        park::withdraw(&self.parked);
        match end {
            Err(_) => core::hint::spin_loop(),
            Ok(WaitEnd::Killed | WaitEnd::NoMemory) => {
                if !interruptible || !current_wait_must_unwind() {
                    crate::block::cache_kernel::park_for_wait_retry();
                }
            }
            Ok(_) => {}
        }
        !interruptible || !current_wait_must_unwind()
    }

    #[cfg(not(test))]
    fn wake(&self) {
        // The exclusion precedes PROCESS_MANAGER in the design 1.1 lock
        // order. Its nested PROCESS_MANAGER section (Mapping::publish's bind
        // from syscall/mmap.rs) ends before the guard drops.
        self.queue.wake_up();
    }

    #[cfg(test)]
    fn park(&self, _interruptible: bool) -> bool {
        self.parked.fetch_add(1, Ordering::SeqCst);
        self.host.parks.fetch_add(1, Ordering::SeqCst);
        let mut sleep = self.host.sleep.lock().unwrap();
        while self.held.load(Ordering::SeqCst) {
            sleep = self
                .host
                .wake
                .wait_timeout(sleep, std::time::Duration::from_millis(50))
                .unwrap()
                .0;
        }
        drop(sleep);
        self.parked.fetch_sub(1, Ordering::SeqCst);
        true
    }

    #[cfg(test)]
    fn wake(&self) {
        let _sleep = self.host.sleep.lock().unwrap();
        self.host.wake.notify_all();
    }

    /// Host checks: contended acquisitions that parked.
    #[cfg(test)]
    pub fn parks(&self) -> u64 {
        self.host.parks.load(Ordering::SeqCst)
    }

    /// Host checks: whether a guard is live.
    #[cfg(test)]
    pub fn is_held(&self) -> bool {
        self.held.load(Ordering::SeqCst)
    }
}

// Host checks: the MountLock guards this thread holds, counted where the
// Acquire and Release events are observed (the I7 guard, design §8 E2.5).
#[cfg(test)]
std::thread_local! {
    static HELD: core::cell::Cell<u32> = const { core::cell::Cell::new(0) };
}

/// Host checks: how many MountLock guards the current thread holds.
#[cfg(test)]
pub fn held_by_current_thread() -> u32 {
    HELD.with(core::cell::Cell::get)
}

#[cfg(test)]
#[derive(Default)]
struct HostPark {
    parks: core::sync::atomic::AtomicU64,
    sleep: std::sync::Mutex<()>,
    wake: std::sync::Condvar,
}

impl<T> Deref for MountGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        #[cfg(test)]
        super::preempt::context::observe(super::preempt::context::Event::Access(self.lock as *const MountLock<T> as usize));
        // SAFETY: a live guard is the only holder of the exclusion.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for MountGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        #[cfg(test)]
        super::preempt::context::observe(super::preempt::context::Event::Access(self.lock as *const MountLock<T> as usize));
        // SAFETY: a live guard is the only holder of the exclusion.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for MountGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.release();
    }
}
