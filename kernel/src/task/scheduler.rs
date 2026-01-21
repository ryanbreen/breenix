//! Preemptive scheduler implementation
//!
//! This module implements a round-robin scheduler for kernel threads.

use super::thread::{Thread, ThreadState};
use crate::log_serial_println;
use alloc::{boxed::Box, collections::VecDeque};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

/// Global scheduler instance
static SCHEDULER: Mutex<Option<Scheduler>> = Mutex::new(None);

/// Global need_resched flag for timer interrupt
static NEED_RESCHED: AtomicBool = AtomicBool::new(false);

/// The kernel scheduler
pub struct Scheduler {
    /// All threads in the system
    threads: alloc::vec::Vec<Box<Thread>>,

    /// Ready queue (thread IDs)
    ready_queue: VecDeque<u64>,

    /// Currently running thread ID
    current_thread: Option<u64>,

    /// Idle thread ID (runs when no other threads are ready)
    idle_thread: u64,
}

impl Scheduler {
    /// Create a new scheduler with an idle thread
    pub fn new(idle_thread: Box<Thread>) -> Self {
        let idle_id = idle_thread.id();
        let scheduler = Self {
            threads: alloc::vec![idle_thread],
            ready_queue: VecDeque::new(),
            current_thread: Some(idle_id),
            idle_thread: idle_id,
        };

        // Don't put idle thread in ready queue
        // It runs only when nothing else is ready

        scheduler
    }

    /// Add a new thread to the scheduler
    pub fn add_thread(&mut self, thread: Box<Thread>) {
        let thread_id = thread.id();
        let thread_name = thread.name.clone();
        let is_user = thread.privilege == super::thread::ThreadPrivilege::User;
        self.threads.push(thread);
        self.ready_queue.push_back(thread_id);
        log_serial_println!(
            "Added thread {} '{}' to scheduler (user: {}, ready_queue: {:?})",
            thread_id,
            thread_name,
            is_user,
            self.ready_queue
        );
    }

    /// Get a mutable thread by ID
    pub fn get_thread_mut(&mut self, id: u64) -> Option<&mut Thread> {
        self.threads
            .iter_mut()
            .find(|t| t.id() == id)
            .map(|t| t.as_mut())
    }

    /// Get the current running thread
    #[allow(dead_code)]
    pub fn current_thread(&self) -> Option<&Thread> {
        self.current_thread.and_then(|id| self.get_thread(id))
    }

    /// Get the current running thread mutably
    pub fn current_thread_mut(&mut self) -> Option<&mut Thread> {
        self.current_thread
            .and_then(move |id| self.get_thread_mut(id))
    }

    /// Schedule the next thread to run
    /// Returns (old_thread, new_thread) for context switching
    pub fn schedule(&mut self) -> Option<(&mut Thread, &Thread)> {
        // Count schedule calls - only log very sparingly to avoid timing issues
        // Serial output is ~960 bytes/sec, so each log line can take 50-100ms!
        static SCHEDULE_COUNT: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        let count = SCHEDULE_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

        // Only log first 5 calls (boot debugging) and then every 500th call
        // CRITICAL: Excessive logging here causes timing issues with kthreads
        let debug_log = count < 5 || (count % 500 == 0);

        // If current thread is still runnable, put it back in ready queue
        if let Some(current_id) = self.current_thread {
            if current_id != self.idle_thread {
                // Check the state and determine what to do
                let (is_terminated, is_blocked) =
                    if let Some(current) = self.get_thread_mut(current_id) {
                        let was_terminated = current.state == ThreadState::Terminated;
                        // Check for any blocked state
                        let was_blocked = current.state == ThreadState::Blocked
                            || current.state == ThreadState::BlockedOnSignal
                            || current.state == ThreadState::BlockedOnChildExit;
                        // Only set to Ready if not terminated AND not blocked
                        if !was_terminated && !was_blocked {
                            current.set_ready();
                        }
                        (was_terminated, was_blocked)
                    } else {
                        (true, false)
                    };

                // Put non-terminated, non-blocked threads back in ready queue
                if !is_terminated && !is_blocked {
                    self.ready_queue.push_back(current_id);
                }
            }
        }

        // Get next thread from ready queue
        let mut next_thread_id = if let Some(n) = self.ready_queue.pop_front() {
            n
        } else {
            self.idle_thread
        };

        if debug_log {
            log_serial_println!(
                "Next thread from queue: {}, ready_queue after pop: {:?}",
                next_thread_id,
                self.ready_queue
            );
        }

        // Important: Don't skip if it's the same thread when there are other threads waiting
        // This was causing the issue where yielding wouldn't switch to other ready threads
        if Some(next_thread_id) == self.current_thread && !self.ready_queue.is_empty() {
            // Put current thread back and get the next one
            self.ready_queue.push_back(next_thread_id);
            next_thread_id = self.ready_queue.pop_front()?;
        } else if Some(next_thread_id) == self.current_thread {
            // Current thread is the only runnable thread.
            // If it's NOT the idle thread, switch to idle to give it a chance.
            // This is important for kthreads that yield while waiting for the idle
            // thread (which runs tests/main logic) to set a flag.
            if next_thread_id != self.idle_thread {
                self.ready_queue.push_back(next_thread_id);
                next_thread_id = self.idle_thread;
                // CRITICAL: Set NEED_RESCHED so the next timer interrupt will
                // switch back to the deferred thread. Without this, idle would
                // spin in HLT for an entire quantum (50ms) before rescheduling.
                crate::per_cpu::set_need_resched(true);
                if debug_log {
                    log_serial_println!(
                        "Thread {} is alone (non-idle), switching to idle {}",
                        self.current_thread.unwrap_or(0),
                        self.idle_thread
                    );
                }
            } else {
                // Idle is the only runnable thread - keep running it.
                // No context switch needed.
                // NOTE: Do NOT push idle to ready_queue here! Idle came from
                // the fallback (line 129), not from pop_front. The ready_queue
                // should remain empty. Pushing idle here would accumulate idle
                // entries in the queue, causing incorrect scheduling when new
                // threads are spawned (the queue would contain both idle AND the
                // new thread, when it should only contain the new thread).
                if debug_log {
                    log_serial_println!(
                        "Idle thread {} is alone, continuing (no switch needed)",
                        next_thread_id
                    );
                }
                return None;
            }
        }

        // If current is idle and we have a real next thread, allow switch even if idle
        let old_thread_id = self.current_thread.unwrap_or(self.idle_thread);
        self.current_thread = Some(next_thread_id);

        if debug_log {
            log_serial_println!(
                "Switching from thread {} to thread {}",
                old_thread_id,
                next_thread_id
            );
        }

        // Mark new thread as running
        if let Some(next) = self.get_thread_mut(next_thread_id) {
            next.set_running();
        }

        // Get mutable reference to old thread and immutable to new
        // This is safe because we know they're different threads
        unsafe {
            let threads_ptr = self.threads.as_mut_ptr();
            let old_idx = self.threads.iter().position(|t| t.id() == old_thread_id)?;
            let new_idx = self.threads.iter().position(|t| t.id() == next_thread_id)?;

            let old_thread = &mut *(*threads_ptr.add(old_idx)).as_mut();
            let new_thread = &*(*threads_ptr.add(new_idx)).as_ref();

            Some((old_thread, new_thread))
        }
    }

    /// Block the current thread
    #[allow(dead_code)]
    pub fn block_current(&mut self) {
        if let Some(current) = self.current_thread_mut() {
            current.set_blocked();
        }
    }

    /// Unblock a thread by ID
    pub fn unblock(&mut self, thread_id: u64) {
        if let Some(thread) = self.get_thread_mut(thread_id) {
            if thread.state == ThreadState::Blocked || thread.state == ThreadState::BlockedOnSignal {
                thread.set_ready();
                if thread_id != self.idle_thread && !self.ready_queue.contains(&thread_id) {
                    self.ready_queue.push_back(thread_id);
                    log_serial_println!("unblock({}): Added to ready_queue", thread_id);
                }
            }
        }
    }

    /// Block current thread until a signal is delivered
    /// Used by the pause() syscall
    ///
    /// NOTE: This does NOT set current_thread to None because the thread
    /// is still physically running the syscall. The schedule() function
    /// will check the thread state and not put it back in ready queue.
    pub fn block_current_for_signal(&mut self) {
        if let Some(current_id) = self.current_thread {
            if let Some(thread) = self.get_thread_mut(current_id) {
                thread.state = ThreadState::BlockedOnSignal;
                // CRITICAL: Mark that this thread is blocked inside a syscall.
                // When the thread is resumed, we must NOT restore userspace context
                // because that would return to the pre-syscall location instead of
                // letting the syscall complete and return properly.
                thread.blocked_in_syscall = true;
                log_serial_println!("Thread {} blocked waiting for signal (blocked_in_syscall=true)", current_id);
            }
            // Remove from ready queue (shouldn't be there but make sure)
            self.ready_queue.retain(|&id| id != current_id);
            // NOTE: Do NOT clear current_thread here!
            // The thread is still running (inside the syscall handler).
            // schedule() will detect the Blocked state and not put it back in ready queue.
        }
    }

    /// Unblock a thread that was waiting for a signal
    /// Called when a signal is delivered to a blocked thread
    pub fn unblock_for_signal(&mut self, thread_id: u64) {
        log_serial_println!(
            "unblock_for_signal: Checking thread {} (current={:?}, ready_queue={:?})",
            thread_id,
            self.current_thread,
            self.ready_queue
        );
        if let Some(thread) = self.get_thread_mut(thread_id) {
            log_serial_println!(
                "unblock_for_signal: Thread {} state is {:?}, blocked_in_syscall={}",
                thread_id,
                thread.state,
                thread.blocked_in_syscall
            );
            if thread.state == ThreadState::BlockedOnSignal {
                thread.set_ready();
                // NOTE: Do NOT clear blocked_in_syscall here!
                // The thread needs to resume inside the syscall and complete it.
                // blocked_in_syscall will be cleared when the syscall actually returns.
                if thread_id != self.idle_thread && !self.ready_queue.contains(&thread_id) {
                    self.ready_queue.push_back(thread_id);
                    log_serial_println!(
                        "unblock_for_signal: Thread {} unblocked, added to ready_queue={:?}",
                        thread_id,
                        self.ready_queue
                    );
                } else {
                    log_serial_println!(
                        "unblock_for_signal: Thread {} already in queue or is idle",
                        thread_id
                    );
                }
            } else {
                log_serial_println!(
                    "unblock_for_signal: Thread {} not BlockedOnSignal, state={:?}",
                    thread_id,
                    thread.state
                );
            }
        } else {
            log_serial_println!("unblock_for_signal: Thread {} not found!", thread_id);
        }
    }

    /// Block current thread until a child exits
    /// Used by the waitpid() syscall
    ///
    /// NOTE: This does NOT set current_thread to None because the thread
    /// is still physically running the syscall. The schedule() function
    /// will check the thread state and not put it back in ready queue.
    pub fn block_current_for_child_exit(&mut self) {
        if let Some(current_id) = self.current_thread {
            if let Some(thread) = self.get_thread_mut(current_id) {
                thread.state = ThreadState::BlockedOnChildExit;
                // CRITICAL: Mark that this thread is blocked inside a syscall.
                // When the thread is resumed, we must NOT restore userspace context
                // because that would return to the pre-syscall location instead of
                // letting the syscall complete and return properly.
                thread.blocked_in_syscall = true;
                log_serial_println!("Thread {} blocked waiting for child exit (blocked_in_syscall=true)", current_id);
            }
            // Remove from ready queue (shouldn't be there but make sure)
            self.ready_queue.retain(|&id| id != current_id);
            // NOTE: Do NOT clear current_thread here!
            // The thread is still running (inside the syscall handler).
            // schedule() will detect the Blocked state and not put it back in ready queue.
        }
    }

    /// Unblock a thread that was waiting for a child to exit
    /// Called when a child process terminates
    pub fn unblock_for_child_exit(&mut self, thread_id: u64) {
        if let Some(thread) = self.get_thread_mut(thread_id) {
            if thread.state == ThreadState::BlockedOnChildExit {
                thread.set_ready();
                if thread_id != self.idle_thread && !self.ready_queue.contains(&thread_id) {
                    self.ready_queue.push_back(thread_id);
                    log_serial_println!("Thread {} unblocked by child exit", thread_id);
                }
            }
        }
    }

    /// Terminate the current thread
    #[allow(dead_code)]
    pub fn terminate_current(&mut self) {
        if let Some(current) = self.current_thread_mut() {
            current.set_terminated();
            // Don't put back in ready queue
        }
        self.current_thread = None;
    }

    /// Check if scheduler has any runnable threads
    pub fn has_runnable_threads(&self) -> bool {
        !self.ready_queue.is_empty()
            || self.current_thread.map_or(false, |id| {
                self.get_thread(id).map_or(false, |t| t.is_runnable())
            })
    }

    /// Check if scheduler has any userspace threads (ready, running, or blocked)
    pub fn has_userspace_threads(&self) -> bool {
        self.threads.iter().any(|t| {
            t.id() != self.idle_thread
                && t.privilege == super::thread::ThreadPrivilege::User
                && t.state != super::thread::ThreadState::Terminated
        })
    }

    /// Remove a thread from the ready queue (used when blocking)
    pub fn remove_from_ready_queue(&mut self, thread_id: u64) {
        self.ready_queue.retain(|&id| id != thread_id);
    }

    /// Get a thread by ID (public for timer.rs)
    pub fn get_thread(&self, id: u64) -> Option<&Thread> {
        self.threads
            .iter()
            .find(|t| t.id() == id)
            .map(|t| t.as_ref())
    }

    /// Get the idle thread ID
    pub fn idle_thread(&self) -> u64 {
        self.idle_thread
    }

    /// Set the current thread (used by spawn mechanism)
    #[allow(dead_code)]
    pub fn set_current_thread(&mut self, thread_id: u64) {
        self.current_thread = Some(thread_id);
    }
}

/// Initialize the global scheduler
#[allow(dead_code)]
pub fn init(idle_thread: Box<Thread>) {
    let mut scheduler_lock = SCHEDULER.lock();
    *scheduler_lock = Some(Scheduler::new(idle_thread));
    log_serial_println!("Scheduler initialized");
}

/// Initialize scheduler with the current thread as the idle task (Linux-style)
/// This is used during boot where the boot thread becomes the idle task
pub fn init_with_current(current_thread: Box<Thread>) {
    let mut scheduler_lock = SCHEDULER.lock();
    let thread_id = current_thread.id();
    
    // Create scheduler with current thread as both idle and current
    let mut scheduler = Scheduler::new(current_thread);
    scheduler.current_thread = Some(thread_id);
    
    *scheduler_lock = Some(scheduler);
    log_serial_println!("Scheduler initialized with current thread {} as idle task", thread_id);
}

/// Add a thread to the scheduler
pub fn spawn(thread: Box<Thread>) {
    // Disable interrupts to prevent timer interrupt deadlock
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            scheduler.add_thread(thread);
            // Ensure a switch happens ASAP (especially in CI smoke runs)
            NEED_RESCHED.store(true, Ordering::Relaxed);
            // Mirror to per-CPU flag so IRQ-exit path sees it
            crate::per_cpu::set_need_resched(true);
        } else {
            panic!("Scheduler not initialized");
        }
    });
}

/// Perform scheduling and return threads to switch between
pub fn schedule() -> Option<(u64, u64)> {
    // Check if interrupts are already disabled (i.e., we're in interrupt context)
    let interrupts_enabled = x86_64::instructions::interrupts::are_enabled();

    let result = if interrupts_enabled {
        // Normal case: disable interrupts to prevent deadlock
        x86_64::instructions::interrupts::without_interrupts(|| {
            let mut scheduler_lock = SCHEDULER.lock();
            if let Some(scheduler) = scheduler_lock.as_mut() {
                scheduler.schedule().map(|(old, new)| (old.id(), new.id()))
            } else {
                None
            }
        })
    } else {
        // Already in interrupt context - don't try to disable interrupts again
        let mut scheduler_lock = SCHEDULER.lock();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            scheduler.schedule().map(|(old, new)| (old.id(), new.id()))
        } else {
            None
        }
    };

    result
}

/// Special scheduling point called from IRQ exit path
/// This is safe to call from IRQ context when returning to user or idle
pub fn preempt_schedule_irq() {
    // IMPORTANT: This function must NOT call schedule()!
    //
    // The schedule() function updates scheduler.current_thread, but the actual
    // context switch only happens on the assembly IRETQ path. Calling schedule()
    // here would desync scheduler state from reality:
    //   1. Thread A is running
    //   2. preempt_schedule_irq calls schedule(), sets current_thread = B
    //   3. We return through softirq_exit -> irq_exit -> timer ISR -> IRETQ
    //   4. IRETQ returns to thread A's context (no switch happened)
    //   5. Scheduler thinks B is running, but A is actually running
    //   6. Next schedule() saves A's regs to B's context -> corruption
    //
    // Instead, we leave need_resched set. The assembly interrupt return path
    // (check_need_resched_and_switch) will:
    //   1. Check need_resched
    //   2. Call schedule() to decide what to switch to
    //   3. Perform the actual context switch before IRETQ
    //
    // See also: yield_current() which similarly just sets need_resched
    // and the ARCHITECTURAL CONSTRAINT comment near schedule().

    // No-op: Let the assembly IRETQ path handle context switching
}

/// Non-blocking scheduling attempt (for interrupt context). Returns None if lock is busy.
/// Note: Currently unused - the assembly interrupt return path handles scheduling.
/// Kept as part of public API for potential future use in SMP context.
#[allow(dead_code)]
pub fn try_schedule() -> Option<(u64, u64)> {
    // Do not disable interrupts; we only attempt a non-blocking lock here
    if let Some(mut scheduler_lock) = SCHEDULER.try_lock() {
        if let Some(scheduler) = scheduler_lock.as_mut() {
            return scheduler.schedule().map(|(old, new)| (old.id(), new.id()));
        }
    }
    None
}

/// Get access to the scheduler
/// This function disables interrupts to prevent deadlock with timer interrupt
pub fn with_scheduler<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut Scheduler) -> R,
{
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        scheduler_lock.as_mut().map(f)
    })
}

/// Get mutable access to a specific thread (for timer interrupt handler)
/// This function disables interrupts to prevent deadlock with timer interrupt
pub fn with_thread_mut<F, R>(thread_id: u64, f: F) -> Option<R>
where
    F: FnOnce(&mut super::thread::Thread) -> R,
{
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        scheduler_lock
            .as_mut()
            .and_then(|sched| sched.get_thread_mut(thread_id).map(f))
    })
}

/// Get the current thread ID
/// This function disables interrupts to prevent deadlock with timer interrupt
pub fn current_thread_id() -> Option<u64> {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let scheduler_lock = SCHEDULER.lock();
        scheduler_lock.as_ref().and_then(|s| s.current_thread)
    })
}

/// Yield the current thread
pub fn yield_current() {
    // CRITICAL FIX: Do NOT call schedule() here!
    // schedule() updates self.current_thread, but no actual context switch happens.
    // This caused the scheduler to get out of sync with reality:
    //   1. Thread A is running
    //   2. yield_current() calls schedule(), returns (A, B), sets current_thread = B
    //   3. No actual context switch - thread A continues running
    //   4. Timer fires, schedule() returns (B, C), saves thread A's regs to thread B's context
    //   5. Thread B's context is now corrupted with thread A's registers
    //
    // Instead, just set need_resched flag. The actual scheduling decision and context
    // switch will happen at the next interrupt return via check_need_resched_and_switch.
    set_need_resched();
}

// NOTE: get_pending_switch() was removed because it called schedule() which mutates
// self.current_thread. Calling it "just to peek" would corrupt scheduler state.
// If needed in future, implement a true peek function that doesn't mutate state.
//
// ARCHITECTURAL CONSTRAINT: Never add a function that calls schedule() "just to look"
// at what would happen. The schedule() function MUST only be called when an actual
// context switch will follow immediately. Violating this invariant will desync
// scheduler.current_thread from reality, causing register corruption in child processes.
// See commit f59bccd for the full bug investigation.

/// Allocate a new thread ID
#[allow(dead_code)]
pub fn allocate_thread_id() -> Option<u64> {
    Some(super::thread::allocate_thread_id())
}

/// Set the need_resched flag (called from timer interrupt)
pub fn set_need_resched() {
    NEED_RESCHED.store(true, Ordering::Relaxed);
    crate::per_cpu::set_need_resched(true);
}

/// Check and clear the need_resched flag (called from interrupt return path)
pub fn check_and_clear_need_resched() -> bool {
    let need = crate::per_cpu::need_resched();
    if need { crate::per_cpu::set_need_resched(false); }
    let _ = NEED_RESCHED.swap(false, Ordering::Relaxed);
    need
}

/// Check if the need_resched flag is set (without clearing it)
/// Used by can_schedule() to determine if kernel threads should be rescheduled
pub fn is_need_resched() -> bool {
    crate::per_cpu::need_resched() || NEED_RESCHED.load(Ordering::Relaxed)
}

/// Switch to idle thread immediately (for use by exception handlers)
/// This updates scheduler state so subsequent timer interrupts can properly schedule.
/// Call this before modifying exception frame to return to idle_loop.
pub fn switch_to_idle() {
    with_scheduler(|sched| {
        let idle_id = sched.idle_thread;
        sched.current_thread = Some(idle_id);

        // Also update per-CPU current thread pointer
        if let Some(thread) = sched.get_thread_mut(idle_id) {
            let thread_ptr = thread as *const _ as *mut crate::task::thread::Thread;
            crate::per_cpu::set_current_thread(thread_ptr);
            log::info!(
                "Exception handler: Set per_cpu thread to idle {} at {:p}",
                idle_id, thread_ptr
            );
        } else {
            log::error!("Exception handler: Failed to get idle thread {} from scheduler!", idle_id);
        }

        log::info!("Exception handler: Switched scheduler to idle thread {}", idle_id);
    });
}

/// Test module for scheduler state invariants
#[cfg(test)]
pub mod tests {
    use super::*;

    /// Test that yield_current() does NOT modify scheduler.current_thread.
    ///
    /// This test validates the fix for the bug where yield_current() called schedule(),
    /// which updated self.current_thread without an actual context switch occurring.
    /// This caused scheduler state to desync from reality, corrupting child process
    /// register state during fork.
    ///
    /// The fix changed yield_current() to only set the need_resched flag, deferring
    /// the actual scheduling decision to the next interrupt return.
    pub fn test_yield_current_does_not_modify_scheduler_state() {
        log::info!("=== TEST: yield_current() scheduler state invariant ===");

        // Capture the current thread ID before yield
        let thread_id_before = current_thread_id();
        log::info!("Thread ID before yield_current(): {:?}", thread_id_before);

        // Call yield_current() - this should ONLY set need_resched flag
        yield_current();

        // Capture the current thread ID after yield
        let thread_id_after = current_thread_id();
        log::info!("Thread ID after yield_current(): {:?}", thread_id_after);

        // CRITICAL ASSERTION: current_thread should NOT have changed
        // If this fails, it means yield_current() is calling schedule() which
        // would cause the register corruption bug to return.
        assert_eq!(
            thread_id_before, thread_id_after,
            "BUG: yield_current() modified scheduler.current_thread! \
             This will cause fork to corrupt child registers. \
             yield_current() must ONLY set need_resched flag, not call schedule()."
        );

        // Verify that need_resched was set
        let need_resched = crate::per_cpu::need_resched();
        assert!(
            need_resched,
            "yield_current() should have set the need_resched flag"
        );

        // Clean up: clear the need_resched flag to avoid affecting other tests
        crate::per_cpu::set_need_resched(false);

        log::info!("=== TEST PASSED: yield_current() correctly preserves scheduler state ===");
    }
}

/// Public wrapper for running scheduler tests (callable from kernel main)
/// This is intentionally available but not automatically called - it can be
/// invoked manually during debugging to verify scheduler invariants.
#[allow(dead_code)]
pub fn run_scheduler_tests() {
    #[cfg(test)]
    {
        tests::test_yield_current_does_not_modify_scheduler_state();
    }
    #[cfg(not(test))]
    {
        // In non-test builds, run a simplified version that doesn't use assert
        log::info!("=== Scheduler invariant check (non-test mode) ===");

        let thread_id_before = current_thread_id();
        yield_current();
        let thread_id_after = current_thread_id();

        if thread_id_before != thread_id_after {
            log::error!(
                "SCHEDULER BUG: yield_current() changed current_thread from {:?} to {:?}!",
                thread_id_before, thread_id_after
            );
        } else {
            log::info!("Scheduler invariant check passed: yield_current() preserves state");
        }

        // Clean up
        crate::per_cpu::set_need_resched(false);
    }
}
