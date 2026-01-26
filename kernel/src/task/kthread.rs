use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::ToString;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use spin::Mutex;

use super::scheduler;
use super::thread::Thread;

// Architecture-specific interrupt control and halt
#[cfg(target_arch = "x86_64")]
use x86_64::instructions::interrupts::without_interrupts;

#[cfg(target_arch = "aarch64")]
use crate::arch_impl::aarch64::cpu::without_interrupts;

/// Architecture-specific halt instruction
#[inline(always)]
fn arch_halt() {
    #[cfg(target_arch = "x86_64")]
    x86_64::instructions::hlt();

    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("wfi", options(nomem, nostack));
    }
}

/// Architecture-specific enable interrupts
#[inline(always)]
unsafe fn arch_enable_interrupts() {
    #[cfg(target_arch = "x86_64")]
    x86_64::instructions::interrupts::enable();

    #[cfg(target_arch = "aarch64")]
    core::arch::asm!("msr daifclr, #2", options(nomem, nostack));
}

/// Kernel thread control block
pub struct Kthread {
    /// Thread ID (same as regular thread)
    pub tid: u64,
    /// Stop flag - thread should check this and exit
    should_stop: AtomicBool,
    /// Exit code set by thread
    exit_code: AtomicI32,
    /// Whether thread has exited
    exited: AtomicBool,
    /// Parked flag for sleep/wake
    parked: AtomicBool,
}

#[derive(Clone)]
pub struct KthreadHandle {
    inner: Arc<Kthread>,
}

impl KthreadHandle {
    /// Get the thread ID of this kthread
    pub fn tid(&self) -> u64 {
        self.inner.tid
    }
}

#[derive(Debug)]
pub enum KthreadError {
    SpawnFailed,
    AlreadyStopped,
    #[allow(dead_code)] // Part of public API, may be used by future kthread operations
    NotFound,
}

struct KthreadStart {
    func: Option<Box<dyn FnOnce() + Send + 'static>>,
}

static KTHREAD_REGISTRY: Mutex<BTreeMap<u64, Arc<Kthread>>> = Mutex::new(BTreeMap::new());

/// Create and immediately start a kernel thread
pub fn kthread_run<F>(func: F, name: &str) -> Result<KthreadHandle, KthreadError>
where
    F: FnOnce() + Send + 'static,
{
    let mut thread =
        Thread::new_kernel(name.to_string(), kthread_entry, 0).map_err(|_| KthreadError::SpawnFailed)?;

    let tid = thread.id;
    let kthread = Arc::new(Kthread {
        tid,
        should_stop: AtomicBool::new(false),
        exit_code: AtomicI32::new(0),
        exited: AtomicBool::new(false),
        parked: AtomicBool::new(false),
    });

    let start = Box::new(KthreadStart {
        func: Some(Box::new(func)),
    });
    // Pass the KthreadStart pointer as the argument to kthread_entry
    // x86_64: argument passed in RDI register (System V ABI)
    // ARM64: argument passed in X0 register (AAPCS64)
    #[cfg(target_arch = "x86_64")]
    {
        thread.context.rdi = Box::into_raw(start) as u64;
    }
    #[cfg(target_arch = "aarch64")]
    {
        thread.context.x0 = Box::into_raw(start) as u64;
    }

    // CRITICAL: Disable interrupts across both registry insert AND spawn to prevent
    // a race where the timer interrupt schedules the new thread before we've finished
    // setting up. The new thread's kthread_entry calls current_kthread() which needs
    // the registry entry to exist.
    without_interrupts(|| {
        KTHREAD_REGISTRY.lock().insert(tid, Arc::clone(&kthread));
        scheduler::spawn(Box::new(thread));
    });

    Ok(KthreadHandle { inner: kthread })
}

/// Signal thread to stop (non-blocking)
pub fn kthread_stop(handle: &KthreadHandle) -> Result<(), KthreadError> {
    if handle.inner.exited.load(Ordering::Acquire) {
        return Err(KthreadError::AlreadyStopped);
    }

    if handle.inner.should_stop.swap(true, Ordering::AcqRel) {
        return Err(KthreadError::AlreadyStopped);
    }

    // Always call unpark to wake the thread. This handles two cases:
    // 1. If parked via kthread_park(), this wakes it immediately
    // 2. If not parked, this is harmless but ensures the thread gets scheduled
    // The kthread should use kthread_park() in its wait loop, not bare HLT,
    // to ensure kthread_stop() can wake it promptly.
    kthread_unpark(handle);

    Ok(())
}

/// Check if current thread should stop (called by kthread function)
pub fn kthread_should_stop() -> bool {
    current_kthread()
        .map(|handle| handle.inner.should_stop.load(Ordering::Acquire))
        .unwrap_or(false)
}

/// Park current thread until unparked (sleep)
/// Use this in kthread wait loops instead of bare HLT to ensure kthread_stop() can wake promptly.
pub fn kthread_park() {
    let handle = match current_kthread() {
        Some(h) => h,
        None => return, // Not a kthread, nothing to do
    };

    // Set parked flag first
    handle.inner.parked.store(true, Ordering::Release);

    // CRITICAL: Check should_stop AFTER setting parked to handle race with kthread_stop().
    // If kthread_stop() was called before we set parked, we need to return immediately.
    // If kthread_stop() is called after we set parked, it will call kthread_unpark().
    if handle.inner.should_stop.load(Ordering::Acquire) {
        handle.inner.parked.store(false, Ordering::Release);
        return;
    }

    // Wait in a loop until we're actually unparked.
    // For kthreads, we use the simple Blocked state (not BlockedOnSignal which
    // is designed for userspace syscalls and has special signal delivery logic).
    while handle.inner.parked.load(Ordering::Acquire) {
        // CRITICAL: Disable interrupts while updating scheduler state to prevent
        // race where timer interrupt fires between marking blocked and removing from queue
        without_interrupts(|| {
            // Re-check parked under interrupt disable to handle race with unpark
            if !handle.inner.parked.load(Ordering::Acquire) {
                return; // Already unparked, don't block
            }

            // Mark thread as Blocked and remove from ready queue
            scheduler::with_scheduler(|sched| {
                if let Some(thread) = sched.current_thread_mut() {
                    thread.state = crate::task::thread::ThreadState::Blocked;
                }
                // Remove from ready queue to ensure scheduler doesn't pick us up
                sched.remove_from_ready_queue(handle.inner.tid);
            });
        });

        // Check again after critical section - unpark might have happened
        if !handle.inner.parked.load(Ordering::Acquire) {
            break;
        }

        // Set need_resched so context switch happens
        scheduler::yield_current();

        // HLT/WFI waits for the next interrupt (timer) which will perform the actual context switch
        arch_halt();
    }
}

/// Unpark a parked thread (wake)
pub fn kthread_unpark(handle: &KthreadHandle) {
    handle.inner.parked.store(false, Ordering::Release);
    scheduler::with_scheduler(|sched| {
        sched.unblock(handle.inner.tid);
    });
    // CRITICAL: Set need_resched to ensure a context switch happens soon.
    // Without this, the unparked thread won't run until the current thread's
    // quantum expires (up to 50ms). This matches spawn()'s behavior of setting
    // need_resched after adding a thread to ready_queue.
    scheduler::set_need_resched();
}

/// Wait for kthread to exit and return its exit code
/// Blocks the calling context until the thread terminates
pub fn kthread_join(handle: &KthreadHandle) -> Result<i32, KthreadError> {
    // Check if already exited - return immediately with exit_code
    // Use SeqCst to match kthread_exit()'s SeqCst store
    if handle.inner.exited.load(Ordering::SeqCst) {
        return Ok(handle.inner.exit_code.load(Ordering::Acquire));
    }

    // Wait for thread to exit using HLT/WFI to allow timer interrupts
    // This lets the scheduler run the kthread to completion
    while !handle.inner.exited.load(Ordering::SeqCst) {
        arch_halt();
    }

    // The SeqCst load above synchronizes with kthread_exit()'s SeqCst store,
    // ensuring we see the exit_code written before the exited flag
    Ok(handle.inner.exit_code.load(Ordering::Acquire))
}

/// Exit the current kthread with a specific exit code.
pub fn kthread_exit(code: i32) -> ! {
    let handle = current_kthread().expect("kthread_exit called outside kthread");

    // Store exit_code BEFORE setting exited flag with a release fence.
    // This ensures kthread_join() sees the exit_code when it observes exited=true.
    handle.inner.exit_code.store(code, Ordering::Release);
    // Use SeqCst for exited to provide a total order with kthread_join()'s acquire load
    handle.inner.exited.store(true, Ordering::SeqCst);
    handle.inner.parked.store(false, Ordering::Release);

    // Remove from registry AFTER setting exited, so kthread_join() can still find us
    KTHREAD_REGISTRY.lock().remove(&handle.inner.tid);

    scheduler::with_scheduler(|sched| {
        if let Some(thread) = sched.current_thread_mut() {
            thread.set_terminated();
        }
    });
    scheduler::set_need_resched();

    loop {
        unsafe { arch_enable_interrupts(); }
        arch_halt();
    }
}

/// Get handle for current kthread (if running in one)
pub fn current_kthread() -> Option<KthreadHandle> {
    let tid = scheduler::current_thread_id()?;
    KTHREAD_REGISTRY
        .lock()
        .get(&tid)
        .cloned()
        .map(|inner| KthreadHandle { inner })
}

extern "C" fn kthread_entry(arg: u64) -> ! {
    // NOTE: No logging in kthread_entry - log statements can cause deadlocks
    // when timer interrupts fire while holding the logger lock. Use raw serial
    // output for debugging only. The KTHREAD_CREATE marker in kthread_run() is
    // sufficient for boot stage verification.

    // CRITICAL: Enable interrupts for kernel threads!
    // Kernel threads are initialized with RFLAGS = 0x002 (IF=0, interrupts disabled)
    // on x86_64, or with DAIF.I=1 (IRQs masked) on ARM64, to prevent preemption
    // during initial setup. Now that we're in the entry point, we need to enable
    // interrupts so timer interrupts can preempt us and the scheduler can switch
    // between threads.
    unsafe {
        arch_enable_interrupts();
    }

    let start = unsafe { Box::from_raw(arg as *mut KthreadStart) };
    let KthreadStart { func } = *start;

    if let Some(func) = func {
        func();
    }

    // If the thread function returns, default to exit_code=0. For custom codes, call kthread_exit(code).
    kthread_exit(0);
}
