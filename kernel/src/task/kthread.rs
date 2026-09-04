use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::ToString;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use spin::Mutex;

use super::scheduler;
use super::thread::Thread;

// Architecture-generic HAL wrappers for interrupt control and halt.
// These delegate to the correct CpuOps implementation for each architecture.
use crate::{
    arch_enable_interrupts, arch_halt, arch_halt_with_interrupts,
    arch_without_interrupts as without_interrupts,
};

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

/// Published park intents that an unpark cleared before the parker slept.
///
/// An increment is a wakeup kept that the older check-then-park shape had no
/// way to keep. Diagnostic, not a fault: 2 of 9 boots in the batch that first
/// carried this counter reported a non-zero value.
static PARK_INTENT_CLEARED_BEFORE_SLEEP: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// Read the raced-park count. See PARK_INTENT_CLEARED_BEFORE_SLEEP.
pub fn park_intents_cleared_before_sleep() -> u64 {
    PARK_INTENT_CLEARED_BEFORE_SLEEP.load(Ordering::Relaxed)
}

/// Create and immediately start a kernel thread
pub fn kthread_run<F>(func: F, name: &str) -> Result<KthreadHandle, KthreadError>
where
    F: FnOnce() + Send + 'static,
{
    kthread_run_with(func, name, scheduler::spawn)
}

/// Create a kernel thread whose per-CPU work requires it to stay on `cpu`.
pub(crate) fn kthread_run_on_cpu<F>(
    func: F,
    name: &str,
    cpu: usize,
) -> Result<KthreadHandle, KthreadError>
where
    F: FnOnce() + Send + 'static,
{
    kthread_run_with(func, name, move |thread| {
        scheduler::spawn_on_cpu(thread, cpu)
    })
}

#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
pub(crate) fn kthread_run_on_cpu_for_test<F>(
    func: F,
    name: &str,
    cpu: usize,
) -> Result<KthreadHandle, KthreadError>
where
    F: FnOnce() + Send + 'static,
{
    kthread_run_with(func, name, move |thread| {
        scheduler::spawn_on_cpu_for_test(thread, cpu)
    })
}

/// Test-only: run a kernel thread published through the production placement
/// path, pinned afterwards to whichever CPU that path selected. The selected
/// CPU is reported through `placed_cpu`.
#[cfg(all(target_arch = "aarch64", feature = "arm_a_609"))]
pub(crate) fn kthread_run_pinned_where_placed_for_test<F>(
    func: F,
    name: &str,
    placed_cpu: &core::sync::atomic::AtomicUsize,
) -> Result<KthreadHandle, KthreadError>
where
    F: FnOnce() + Send + 'static,
{
    kthread_run_with(func, name, move |thread| {
        placed_cpu.store(
            scheduler::spawn_pinned_where_placed_for_test(thread),
            Ordering::Release,
        );
    })
}

fn kthread_run_with<F, S>(func: F, name: &str, spawn_thread: S) -> Result<KthreadHandle, KthreadError>
where
    F: FnOnce() + Send + 'static,
    S: FnOnce(Box<Thread>),
{
    let mut thread = Thread::new_kernel(name.to_string(), kthread_entry, 0)
        .map_err(|_| KthreadError::SpawnFailed)?;

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
        spawn_thread(Box::new(thread));
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

/// Publish the current kthread's intent to park, without sleeping yet.
///
/// Returns false in the 2 cases where a park would be wrong -- the caller is
/// not a kthread, or a stop was already requested -- and leaves 0 intent
/// published.
///
/// This is the publish half of the protocol Linux spells prepare_to_wait:
/// publish the sleep intent, THEN re-check the condition, then sleep. A
/// producer that makes work available between a consumer's last check and its
/// sleep can only cancel that sleep if the intent is already visible, so a
/// consumer that checks its condition first and parks afterwards loses that
/// wakeup.
/// claim-lint:ok: the 2 orders this closes are the 2 counters above and in
/// kernel/src/task/workqueue.rs.
pub fn kthread_prepare_to_park() -> bool {
    let handle = match current_kthread() {
        Some(h) => h,
        None => return false, // Not a kthread, nothing to do
    };

    // Set parked flag first
    handle.inner.parked.store(true, Ordering::Release);

    // CRITICAL: Check should_stop AFTER setting parked to handle race with kthread_stop().
    // If kthread_stop() was called before we set parked, we need to return immediately.
    // If kthread_stop() is called after we set parked, it will call kthread_unpark().
    if handle.inner.should_stop.load(Ordering::Acquire) {
        handle.inner.parked.store(false, Ordering::Release);
        return false;
    }

    true
}

/// Abandon a park intent published by kthread_prepare_to_park.
///
/// The caller re-checked its condition, found work, and is not going to sleep.
pub fn kthread_cancel_park() {
    if let Some(handle) = current_kthread() {
        handle.inner.parked.store(false, Ordering::Release);
    }
}

/// Sleep out a park intent published by kthread_prepare_to_park.
///
/// Returns at once if the intent was already cleared -- that is the wakeup
/// this protocol keeps, and it is counted.
pub fn kthread_park_prepared() {
    let handle = match current_kthread() {
        Some(h) => h,
        None => return, // Not a kthread, nothing to do
    };

    if !handle.inner.parked.load(Ordering::Acquire) {
        PARK_INTENT_CLEARED_BEFORE_SLEEP.fetch_add(1, Ordering::Relaxed);
        return;
    }

    // Wait in a loop until we're actually unparked.
    // For kthreads, we use the simple Blocked state (not BlockedOnSignal which
    // is designed for userspace syscalls and has special signal delivery logic).
    while handle.inner.parked.load(Ordering::Acquire) {
        // The parked flag and the scheduler state are ONE decision, so they are
        // read and written under ONE lock. Without that, an unpark can clear
        // the flag and find this thread still Running -- so its unblock has 0
        // state to change -- and this thread then publishes Blocked anyway and is
        // stranded: dequeued, awake for a few more instructions, and gone at
        // the next preemption with nobody left to wake it. Linux serialises
        // task state against wakeups with the runqueue lock for this reason.
        scheduler::with_scheduler(|sched| {
            if !handle.inner.parked.load(Ordering::Acquire) {
                return; // Already unparked under this lock, don't block
            }
            // Mark thread as Blocked and remove from ready queue.
            // The publication goes through the inventoried block_current
            // primitive so 0 blocked state is written outside the family, and
            // the departure comes with it: the primitive removes the current
            // thread from each per-CPU ready queue in the same acquisition.
            sched.block_current();
        });

        // Check again after the critical section - unpark might have happened,
        // and if it did it ran under the same lock, so this thread is Ready.
        if !handle.inner.parked.load(Ordering::Acquire) {
            break;
        }

        // Set need_resched so context switch happens
        scheduler::yield_current();

        // HLT/WFI waits for the next interrupt (timer) which will perform the actual context switch
        arch_halt_with_interrupts();
    }
}

/// Park current thread until unparked (sleep)
/// Use this in kthread wait loops instead of bare HLT to ensure kthread_stop() can wake promptly.
///
/// Publish-then-sleep with no re-check between the halves. A caller whose wake
/// condition can become true between its own last check and this call must use
/// the three-part form instead: kthread_prepare_to_park, the re-check, then
/// kthread_park_prepared or kthread_cancel_park.
pub fn kthread_park() {
    if kthread_prepare_to_park() {
        kthread_park_prepared();
    }
}

/// Unpark a parked thread (wake)
///
/// The flag clear and the unblock are ONE decision and are made under ONE
/// lock, the same lock kthread_park_prepared publishes Blocked under. Clearing
/// the flag outside it lets a parker read the cleared flag, publish Blocked
/// anyway, and be left with 0 wakeups coming.
pub fn kthread_unpark(handle: &KthreadHandle) {
    scheduler::with_scheduler(|sched| {
        handle.inner.parked.store(false, Ordering::Release);
        sched.unblock(handle.inner.tid);
    });
    // CRITICAL: Set need_resched to ensure a context switch happens soon.
    // Without this, the unparked thread will not run until the current
    // quantum expires (up to 50ms). This matches spawn()'s behavior of setting
    // need_resched after adding a thread to ready_queue.
    scheduler::set_need_resched();
}

/// Test-only, non-blocking probe of whether a kthread has exited.
///
/// Boot-test coordinators (which run synchronously on CPU 0) use this to bound
/// their cross-CPU join waits: they poll this flag with their own capped
/// spin-loop instead of blocking unboundedly in `kthread_join`, so a lost
/// wakeup on a secondary CPU surfaces as an actionable test failure rather than
/// a silent harness hang. This is a pure read; it changes no production
/// behavior and matches the SeqCst ordering `kthread_join` uses.
/// The arch term is load-bearing: every call site is aarch64-gated, and widening
/// it dirties the x86 `boot_tests` build.
#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
pub(crate) fn kthread_has_exited_for_test(handle: &KthreadHandle) -> bool {
    handle.inner.exited.load(Ordering::SeqCst)
}

/// Wait for kthread to exit and return its exit code
/// Blocks the calling context until the thread terminates
pub fn kthread_join(handle: &KthreadHandle) -> Result<i32, KthreadError> {
    // Check if already exited - return immediately with exit_code
    // Use SeqCst to match kthread_exit()'s SeqCst store
    if handle.inner.exited.load(Ordering::SeqCst) {
        return Ok(handle.inner.exit_code.load(Ordering::Acquire));
    }

    // Wait for the thread to exit. The halt UNMASKS interrupts first, and that
    // is load-bearing: a join waits for another thread, so this CPU has to be
    // able to take a timer interrupt and switch to it.
    // A masked wfi loop returns at once on a pending interrupt without taking
    // it, so if the joined thread sits on this CPU -- a CPU-pinned
    // kthread joined by a thread the scheduler placed on the same CPU -- it
    // gets 0 dispatches. Measured: DAIF I set, CPU 1 spinning here, and the
    // pinned softirq-test kthread runnable on CPU 1 with 0 dispatches.
    while !handle.inner.exited.load(Ordering::SeqCst) {
        arch_halt_with_interrupts();
    }

    // The SeqCst load above synchronizes with kthread_exit()'s SeqCst store,
    // ensuring we see the exit_code written before the exited flag
    Ok(handle.inner.exit_code.load(Ordering::Acquire))
}

/// Exit the current kthread with a specific exit code.
pub fn kthread_exit(code: i32) -> ! {
    let handle = current_kthread().expect("kthread_exit called outside kthread");
    #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
    crate::tracing::providers::teardown::record_kthread_exit_stage_for_test(handle.inner.tid);

    // Store exit_code BEFORE setting exited flag with a release fence.
    // This ensures kthread_join() sees the exit_code when it observes exited=true.
    handle.inner.exit_code.store(code, Ordering::Release);
    handle.inner.parked.store(false, Ordering::Release);

    // CRITICAL: Perform all lock-protected cleanup BEFORE setting exited=true.
    // kthread_join() returns as soon as it sees exited=true, and the caller may
    // immediately call kthread_run() which acquires KTHREAD_REGISTRY and SCHEDULER
    // locks with interrupts disabled. If we set exited=true first and then try to
    // acquire these locks with interrupts enabled, we can be preempted while
    // holding them — and the caller's kthread_run (with IRQs disabled) deadlocks.
    //
    // Use without_interrupts to match kthread_run's locking pattern and prevent
    // preemption while holding KTHREAD_REGISTRY lock.
    without_interrupts(|| {
        KTHREAD_REGISTRY.lock().remove(&handle.inner.tid);
    });
    #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
    crate::tracing::providers::teardown::record_kthread_exit_stage_for_test(handle.inner.tid);

    #[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
    scheduler::clear_cpu_affinity_for_test(handle.inner.tid);

    // Keep termination and exit publication interrupt-free: a scheduling pass
    // between them would strand the terminated thread forever, so kthread_join()
    // could never return. Nothing is reordered: exited remains last, after every
    // lock-protected cleanup step, preserving the documented ordering contract
    // with kthread_join() and kthread_run(). This region is only a few stores plus
    // one already-masked scheduler-lock acquisition, so interrupt latency is
    // unaffected.
    without_interrupts(|| {
        scheduler::with_scheduler(|sched| {
            if let Some(thread) = sched.current_thread_mut() {
                thread.set_terminated();
            }
        });
        #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
        crate::tracing::providers::teardown::record_kthread_exit_stage_for_test(handle.inner.tid);
        scheduler::set_need_resched();

        // Set exited LAST — after all lock-protected cleanup is done.
        // Use SeqCst to provide a total order with kthread_join()'s acquire load.
        handle.inner.exited.store(true, Ordering::SeqCst);
        #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
        crate::tracing::providers::teardown::record_kthread_exit_stage_for_test(handle.inner.tid);
    });

    loop {
        unsafe {
            arch_enable_interrupts();
        }
        arch_halt();
    }
}

/// Get handle for current kthread (if running in one)
pub fn current_kthread() -> Option<KthreadHandle> {
    let tid = scheduler::current_thread_id()?;
    // Disable interrupts while holding KTHREAD_REGISTRY to match kthread_run's
    // locking pattern. Without this, a timer interrupt could preempt us while
    // holding the lock, and another thread calling kthread_run (with IRQs disabled)
    // would deadlock trying to acquire the same lock.
    without_interrupts(|| {
        KTHREAD_REGISTRY
            .lock()
            .get(&tid)
            .cloned()
            .map(|inner| KthreadHandle { inner })
    })
}

extern "C" fn kthread_entry(arg: u64) -> ! {
    // NOTE: No logging in kthread_entry - log statements can cause deadlocks
    // when timer interrupts fire while holding the logger lock. Use raw serial
    // output for debugging only. The KTHREAD_CREATE marker in kthread_run() is
    // sufficient for boot stage verification.

    // Breadcrumb 70: CPU 0 reached kthread_entry after ERET dispatch
    #[cfg(target_arch = "aarch64")]
    {
        let cpu_id = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
        if cpu_id == 0 {
            crate::arch_impl::aarch64::timer_interrupt::CPU0_BREADCRUMB_ID
                .store(70, core::sync::atomic::Ordering::Relaxed);
        }
    }

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
