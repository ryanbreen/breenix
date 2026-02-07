//! Preemptive scheduler implementation
//!
//! This module implements a round-robin scheduler for kernel threads.

use super::thread::{Thread, ThreadState};
use crate::log_serial_println;
use alloc::{boxed::Box, collections::VecDeque};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

// Architecture-specific interrupt control
#[cfg(target_arch = "x86_64")]
use x86_64::instructions::interrupts::{are_enabled, without_interrupts};

#[cfg(target_arch = "aarch64")]
use crate::arch_impl::aarch64::cpu::{interrupts_enabled as are_enabled, without_interrupts};

/// Global scheduler instance
static SCHEDULER: Mutex<Option<Scheduler>> = Mutex::new(None);

/// Global need_resched flag for timer interrupt
static NEED_RESCHED: AtomicBool = AtomicBool::new(false);

/// Counter for unblock() calls - used for testing pipe wake mechanism
/// This is a global atomic because:
/// 1. unblock() is called via with_scheduler() which already holds the scheduler lock
/// 2. Tests need to read this outside the scheduler lock
/// 3. AtomicU64 ensures visibility across threads without additional locking
static UNBLOCK_CALL_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Get the current unblock() call count (for testing)
///
/// This function is used by the test framework to verify that pipe wake
/// mechanisms actually call scheduler.unblock(). It's only called when
/// the boot_tests feature is enabled.
#[allow(dead_code)] // Used by test_framework when boot_tests feature is enabled
pub fn unblock_call_count() -> u64 {
    UNBLOCK_CALL_COUNT.load(Ordering::SeqCst)
}

/// Maximum CPUs for scheduler state arrays.
#[cfg(target_arch = "aarch64")]
const MAX_CPUS: usize = 8;
#[cfg(not(target_arch = "aarch64"))]
const MAX_CPUS: usize = 1;

/// Per-CPU scheduler state.
struct CpuSchedulerState {
    /// Currently running thread ID on this CPU
    current_thread: Option<u64>,
    /// Idle thread ID for this CPU
    idle_thread: u64,
}

/// The kernel scheduler
pub struct Scheduler {
    /// All threads in the system
    threads: alloc::vec::Vec<Box<Thread>>,

    /// Ready queue (thread IDs)
    ready_queue: VecDeque<u64>,

    /// Per-CPU scheduler state (current_thread + idle_thread per CPU)
    cpu_state: [CpuSchedulerState; MAX_CPUS],
}

impl Scheduler {
    /// Create a new scheduler with an idle thread for CPU 0.
    pub fn new(idle_thread: Box<Thread>) -> Self {
        let idle_id = idle_thread.id();

        // Initialize all CPU states: CPU 0 gets the idle thread, rest are empty
        const EMPTY_STATE: CpuSchedulerState = CpuSchedulerState {
            current_thread: None,
            idle_thread: 0,
        };
        let mut cpu_state = [EMPTY_STATE; MAX_CPUS];
        cpu_state[0] = CpuSchedulerState {
            current_thread: Some(idle_id),
            idle_thread: idle_id,
        };

        let scheduler = Self {
            threads: alloc::vec![idle_thread],
            ready_queue: VecDeque::new(),
            cpu_state,
        };

        scheduler
    }

    // -------------------------------------------------------------------------
    // Per-CPU state accessors (backward-compatible with single-CPU code)
    // -------------------------------------------------------------------------

    /// Get the current CPU ID for scheduler operations.
    #[inline]
    fn current_cpu_id() -> usize {
        #[cfg(target_arch = "aarch64")]
        {
            crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            0
        }
    }

    /// Register an idle thread for a specific CPU.
    /// Called during secondary CPU bringup to set up per-CPU idle tasks.
    #[cfg(target_arch = "aarch64")]
    pub fn register_idle_thread(&mut self, cpu_id: usize, idle_thread: Box<Thread>) {
        if cpu_id >= MAX_CPUS {
            return;
        }
        let idle_id = idle_thread.id();
        self.threads.push(idle_thread);
        self.cpu_state[cpu_id].idle_thread = idle_id;
        self.cpu_state[cpu_id].current_thread = Some(idle_id);
    }

    /// Add a new thread to the scheduler
    pub fn add_thread(&mut self, thread: Box<Thread>) {
        let thread_id = thread.id();
        let thread_name = thread.name.clone();
        let is_user = thread.privilege == super::thread::ThreadPrivilege::User;
        self.threads.push(thread);
        self.ready_queue.push_back(thread_id);
        // CRITICAL: Only log on x86_64. On ARM64, log_serial_println! uses the same
        // SERIAL1 lock as serial_println!, causing deadlock if timer fires while
        // boot code is printing.
        #[cfg(target_arch = "x86_64")]
        log_serial_println!(
            "Added thread {} '{}' to scheduler (user: {}, ready_queue: {:?})",
            thread_id,
            thread_name,
            is_user,
            self.ready_queue
        );
        #[cfg(not(target_arch = "x86_64"))]
        let _ = (thread_id, thread_name, is_user);
    }

    /// Add a thread as the current running thread without scheduling.
    ///
    /// Used when manually starting the first userspace thread (init process).
    /// The thread is added to the scheduler's thread list and marked as current,
    /// but NOT added to the ready queue. This avoids the scheduler trying to
    /// reschedule when timer interrupts fire.
    #[allow(dead_code)]
    pub fn add_thread_as_current(&mut self, mut thread: Box<Thread>) {
        let thread_id = thread.id();
        let thread_name = thread.name.clone();
        // Mark thread as running
        thread.state = ThreadState::Running;
        thread.has_started = true;
        self.threads.push(thread);
        self.cpu_state[Self::current_cpu_id()].current_thread = Some(thread_id);
        // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
        #[cfg(target_arch = "x86_64")]
        log_serial_println!(
            "Added thread {} '{}' as current (not in ready_queue)",
            thread_id,
            thread_name,
        );
        #[cfg(not(target_arch = "x86_64"))]
        let _ = (thread_id, thread_name);
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
        self.cpu_state[Self::current_cpu_id()].current_thread.and_then(|id| self.get_thread(id))
    }

    /// Get the current running thread mutably
    pub fn current_thread_mut(&mut self) -> Option<&mut Thread> {
        self.cpu_state[Self::current_cpu_id()].current_thread
            .and_then(move |id| self.get_thread_mut(id))
    }

    /// Get the current thread ID
    #[allow(dead_code)]
    pub fn current_thread_id_inner(&self) -> Option<u64> {
        self.cpu_state[Self::current_cpu_id()].current_thread
    }

    /// Get the idle thread ID
    #[allow(dead_code)]
    pub fn idle_thread_id(&self) -> u64 {
        self.cpu_state[Self::current_cpu_id()].idle_thread
    }

    /// Schedule the next thread to run
    /// Returns (old_thread, new_thread) for context switching
    pub fn schedule(&mut self) -> Option<(&mut Thread, &Thread)> {
        // Count schedule calls - only log very sparingly to avoid timing issues
        // Serial output is ~960 bytes/sec, so each log line can take 50-100ms!
        static SCHEDULE_COUNT: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        let _count = SCHEDULE_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

        // CRITICAL: Logging disabled on ARM64 - schedule() is called from context switch
        // path which may be holding the serial lock. On ARM64, log_serial_println! uses
        // the same SERIAL1 lock as serial_println!, causing deadlock if timer fires
        // while boot code is printing.
        // On x86_64, log_serial goes to a separate UART (COM2), so it's safe.
        #[cfg(target_arch = "x86_64")]
        let debug_log = _count < 5 || (_count % 500 == 0);
        #[cfg(not(target_arch = "x86_64"))]
        let debug_log = false;

        // If current thread is still runnable, put it back in ready queue
        if let Some(current_id) = self.cpu_state[Self::current_cpu_id()].current_thread {
            if current_id != self.cpu_state[Self::current_cpu_id()].idle_thread {
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
                // CRITICAL: Check for duplicates! If unblock() already added this thread
                // (e.g., packet arrived during blocking recvfrom), don't add it again.
                // Duplicates cause schedule() to spin when same thread keeps getting selected.
                let in_queue = self.ready_queue.contains(&current_id);
                let will_add = !is_terminated && !is_blocked && !in_queue;

                if will_add {
                    self.ready_queue.push_back(current_id);
                }
            }
        }

        // Get next thread from ready queue
        let mut next_thread_id = if let Some(n) = self.ready_queue.pop_front() {
            n
        } else {
            self.cpu_state[Self::current_cpu_id()].idle_thread
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
        if Some(next_thread_id) == self.cpu_state[Self::current_cpu_id()].current_thread && !self.ready_queue.is_empty() {
            // Put current thread back and get the next one
            self.ready_queue.push_back(next_thread_id);
            next_thread_id = self.ready_queue.pop_front()?;
        } else if Some(next_thread_id) == self.cpu_state[Self::current_cpu_id()].current_thread {
            // Current thread is the only runnable thread.
            // If it's NOT the idle thread, switch to idle to give it a chance.
            // This is important for kthreads that yield while waiting for the idle
            // thread (which runs tests/main logic) to set a flag.
            if next_thread_id != self.cpu_state[Self::current_cpu_id()].idle_thread {
                // On ARM64, don't switch userspace threads to idle. Idle runs in kernel
                // mode (EL1), and ARM64 only preempts when returning to userspace (from_el0=true).
                // If we switched a userspace thread to idle, idle would never be preempted
                // back to the userspace thread because timer fires with from_el0=false.
                #[cfg(target_arch = "aarch64")]
                {
                    let is_userspace = self
                        .get_thread(next_thread_id)
                        .map(|t| t.privilege == super::thread::ThreadPrivilege::User)
                        .unwrap_or(false);
                    if is_userspace {
                        // Userspace thread is alone - keep running it, don't switch to idle
                        if debug_log {
                            log_serial_println!(
                                "Thread {} is userspace and alone, continuing (no idle switch)",
                                next_thread_id
                            );
                        }
                        return None;
                    }
                }
                self.ready_queue.push_back(next_thread_id);
                next_thread_id = self.cpu_state[Self::current_cpu_id()].idle_thread;
                // CRITICAL: Set NEED_RESCHED so the next timer interrupt will
                // switch back to the deferred thread. Without this, idle would
                // spin in HLT for an entire quantum (50ms) before rescheduling.
                #[cfg(target_arch = "x86_64")]
                crate::per_cpu::set_need_resched(true);
                #[cfg(target_arch = "aarch64")]
                crate::per_cpu_aarch64::set_need_resched(true);
                if debug_log {
                    log_serial_println!(
                        "Thread {} is alone (non-idle), switching to idle {}",
                        self.cpu_state[Self::current_cpu_id()].current_thread.unwrap_or(0),
                        self.cpu_state[Self::current_cpu_id()].idle_thread
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
        let old_thread_id = self.cpu_state[Self::current_cpu_id()].current_thread.unwrap_or(self.cpu_state[Self::current_cpu_id()].idle_thread);
        self.cpu_state[Self::current_cpu_id()].current_thread = Some(next_thread_id);

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
        // Increment the call counter for testing (tracks that unblock was called)
        UNBLOCK_CALL_COUNT.fetch_add(1, Ordering::SeqCst);

        if let Some(thread) = self.get_thread_mut(thread_id) {
            if thread.state == ThreadState::Blocked || thread.state == ThreadState::BlockedOnSignal {
                thread.set_ready();

                // SMP safety: Don't add to ready_queue if thread is currently
                // running on any CPU. If a thread is blocked in a syscall's WFI
                // loop (e.g., sys_read waiting for keyboard input), it's still
                // the "current thread" on that CPU. Adding it to the ready_queue
                // would allow another CPU to schedule it simultaneously, causing
                // double-scheduling: two CPUs executing the same thread with the
                // same stack, leading to context corruption and crashes (ELR=0x0).
                // The CPU running the thread will detect the state change (Blocked
                // → Ready) when its WFI loop checks the thread state after waking.
                let is_current_on_any_cpu = (0..MAX_CPUS).any(|cpu| {
                    self.cpu_state[cpu].current_thread == Some(thread_id)
                });

                if !is_current_on_any_cpu
                    && thread_id != self.cpu_state[Self::current_cpu_id()].idle_thread
                    && !self.ready_queue.contains(&thread_id)
                {
                    self.ready_queue.push_back(thread_id);
                    // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
                    #[cfg(target_arch = "x86_64")]
                    log_serial_println!("unblock({}): Added to ready_queue", thread_id);

                    // Send IPI to wake an idle CPU so it can pick up the unblocked thread
                    #[cfg(target_arch = "aarch64")]
                    self.send_resched_ipi();
                }
            }
        }
    }

    /// Send a reschedule IPI (SGI 0) to an idle CPU.
    ///
    /// Called after adding a thread to the ready queue to wake a CPU that's
    /// sitting in WFI so it can pick up the newly-runnable thread.
    /// Only sends to one idle CPU (the first one found) to avoid thundering herd.
    #[cfg(target_arch = "aarch64")]
    fn send_resched_ipi(&self) {
        use crate::arch_impl::aarch64::smp;

        let current_cpu = Self::current_cpu_id();
        let online = smp::cpus_online() as usize;

        for cpu in 0..online {
            if cpu == current_cpu {
                continue;
            }
            // Check if this CPU is running its idle thread
            if cpu < MAX_CPUS {
                if let Some(current) = self.cpu_state[cpu].current_thread {
                    if current == self.cpu_state[cpu].idle_thread {
                        // This CPU is idle - send it a reschedule IPI
                        crate::arch_impl::aarch64::gic::send_sgi(
                            crate::arch_impl::aarch64::constants::SGI_RESCHEDULE as u8,
                            cpu as u8,
                        );
                        return; // Only wake one CPU
                    }
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
        self.block_current_for_signal_with_context(None)
    }

    /// Block current thread until a signal is delivered, saving userspace context
    /// Used by the pause() syscall
    ///
    /// CRITICAL: This version atomically saves the userspace context AND sets
    /// blocked_in_syscall=true under the same scheduler lock. This prevents
    /// a race condition where a signal could arrive after the context is saved
    /// to process.main_thread but before blocked_in_syscall is set.
    ///
    /// The saved_userspace_context on the SCHEDULER's Thread is the single source
    /// of truth for signal delivery - context_switch.rs reads from here, not from
    /// process.main_thread.
    ///
    /// NOTE: This does NOT set current_thread to None because the thread
    /// is still physically running the syscall. The schedule() function
    /// will check the thread state and not put it back in ready queue.
    pub fn block_current_for_signal_with_context(
        &mut self,
        userspace_context: Option<super::thread::CpuContext>,
    ) {
        if let Some(current_id) = self.cpu_state[Self::current_cpu_id()].current_thread {
            if let Some(thread) = self.get_thread_mut(current_id) {
                // CRITICAL: Save userspace context FIRST, THEN set state.
                // This ensures that when unblock_for_signal() is called,
                // the context is already saved and ready for signal delivery.
                if let Some(ctx) = userspace_context {
                    thread.saved_userspace_context = Some(ctx);
                    // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
                    #[cfg(target_arch = "x86_64")]
                    log_serial_println!(
                        "Thread {} saving userspace context: RIP={:#x}",
                        current_id,
                        thread.saved_userspace_context.as_ref().unwrap().rip
                    );
                    // ARM64: No logging - would cause deadlock
                }
                thread.state = ThreadState::BlockedOnSignal;
                // CRITICAL: Mark that this thread is blocked inside a syscall.
                // When the thread is resumed, we must NOT restore userspace context
                // because that would return to the pre-syscall location instead of
                // letting the syscall complete and return properly.
                thread.blocked_in_syscall = true;
                // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
                #[cfg(target_arch = "x86_64")]
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
    ///
    /// NOTE: This function sets the need_resched flag when a thread is successfully
    /// unblocked to ensure it gets scheduled promptly. This is critical for pause()
    /// to wake up in a timely manner when a signal arrives.
    pub fn unblock_for_signal(&mut self, thread_id: u64) {
        // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
        #[cfg(target_arch = "x86_64")]
        log_serial_println!(
            "unblock_for_signal: Checking thread {} (current={:?}, ready_queue={:?})",
            thread_id,
            self.cpu_state[Self::current_cpu_id()].current_thread,
            self.ready_queue
        );
        if let Some(thread) = self.get_thread_mut(thread_id) {
            #[cfg(target_arch = "x86_64")]
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

                // SMP safety: Don't add to ready_queue if thread is current on any CPU
                // (same rationale as unblock() - prevents double-scheduling)
                let is_current_on_any_cpu = (0..MAX_CPUS).any(|cpu| {
                    self.cpu_state[cpu].current_thread == Some(thread_id)
                });

                if !is_current_on_any_cpu
                    && thread_id != self.cpu_state[Self::current_cpu_id()].idle_thread
                    && !self.ready_queue.contains(&thread_id)
                {
                    self.ready_queue.push_back(thread_id);
                    #[cfg(target_arch = "x86_64")]
                    log_serial_println!(
                        "unblock_for_signal: Thread {} unblocked, added to ready_queue={:?}",
                        thread_id,
                        self.ready_queue
                    );

                    // Send IPI to wake an idle CPU
                    #[cfg(target_arch = "aarch64")]
                    self.send_resched_ipi();
                } else {
                    #[cfg(target_arch = "x86_64")]
                    log_serial_println!(
                        "unblock_for_signal: Thread {} already in queue, is idle, or is current on a CPU",
                        thread_id
                    );
                }
                // CRITICAL: Request reschedule so the unblocked thread can run promptly.
                // Without this, the thread is added to ready queue but the scheduler
                // doesn't know to switch to it, causing pause() to timeout waiting for
                // the next timer tick instead of waking up immediately.
                set_need_resched();
            } else {
                #[cfg(target_arch = "x86_64")]
                log_serial_println!(
                    "unblock_for_signal: Thread {} not BlockedOnSignal, state={:?}",
                    thread_id,
                    thread.state
                );
            }
        } else {
            #[cfg(target_arch = "x86_64")]
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
        if let Some(current_id) = self.cpu_state[Self::current_cpu_id()].current_thread {
            if let Some(thread) = self.get_thread_mut(current_id) {
                thread.state = ThreadState::BlockedOnChildExit;
                // CRITICAL: Mark that this thread is blocked inside a syscall.
                // When the thread is resumed, we must NOT restore userspace context
                // because that would return to the pre-syscall location instead of
                // letting the syscall complete and return properly.
                thread.blocked_in_syscall = true;
                // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
                #[cfg(target_arch = "x86_64")]
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
    ///
    /// NOTE: This function sets the need_resched flag when a thread is successfully
    /// unblocked to ensure it gets scheduled promptly. This is critical for waitpid()
    /// to wake up in a timely manner when a child exits.
    pub fn unblock_for_child_exit(&mut self, thread_id: u64) {
        if let Some(thread) = self.get_thread_mut(thread_id) {
            if thread.state == ThreadState::BlockedOnChildExit {
                thread.set_ready();

                // SMP safety: Don't add to ready_queue if thread is current on any CPU
                // (same rationale as unblock() - prevents double-scheduling)
                let is_current_on_any_cpu = (0..MAX_CPUS).any(|cpu| {
                    self.cpu_state[cpu].current_thread == Some(thread_id)
                });

                if !is_current_on_any_cpu
                    && thread_id != self.cpu_state[Self::current_cpu_id()].idle_thread
                    && !self.ready_queue.contains(&thread_id)
                {
                    self.ready_queue.push_back(thread_id);
                    // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
                    #[cfg(target_arch = "x86_64")]
                    log_serial_println!("Thread {} unblocked by child exit", thread_id);

                    // Send IPI to wake an idle CPU
                    #[cfg(target_arch = "aarch64")]
                    self.send_resched_ipi();
                }
                // CRITICAL: Request reschedule so the unblocked thread can run promptly.
                // Without this, the thread is added to ready queue but the scheduler
                // doesn't know to switch to it, causing waitpid() to hang.
                set_need_resched();
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
        self.cpu_state[Self::current_cpu_id()].current_thread = None;
    }

    /// Check if scheduler has any runnable threads
    pub fn has_runnable_threads(&self) -> bool {
        !self.ready_queue.is_empty()
            || self.cpu_state[Self::current_cpu_id()].current_thread.map_or(false, |id| {
                self.get_thread(id).map_or(false, |t| t.is_runnable())
            })
    }

    /// Check if scheduler has any userspace threads (ready, running, or blocked)
    pub fn has_userspace_threads(&self) -> bool {
        self.threads.iter().any(|t| {
            // Exclude all idle threads (one per CPU)
            !self.cpu_state.iter().any(|cs| cs.idle_thread == t.id())
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
        self.cpu_state[Self::current_cpu_id()].idle_thread
    }

    /// Get the current thread ID for a specific CPU (for diagnostics).
    /// Used by ARM64 exception handler to dump per-CPU state on crash.
    #[cfg(target_arch = "aarch64")]
    pub fn current_thread_for_cpu(&self, cpu: usize) -> Option<u64> {
        if cpu < MAX_CPUS {
            self.cpu_state[cpu].current_thread
        } else {
            None
        }
    }

    /// Set the current thread (used by spawn mechanism)
    #[allow(dead_code)]
    pub fn set_current_thread(&mut self, thread_id: u64) {
        self.cpu_state[Self::current_cpu_id()].current_thread = Some(thread_id);
    }
}

/// Initialize the global scheduler
#[allow(dead_code)]
pub fn init(idle_thread: Box<Thread>) {
    let mut scheduler_lock = SCHEDULER.lock();
    *scheduler_lock = Some(Scheduler::new(idle_thread));
    // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
    #[cfg(target_arch = "x86_64")]
    log_serial_println!("Scheduler initialized");
}

/// Initialize scheduler with the current thread as the idle task (Linux-style)
/// This is used during boot where the boot thread becomes the idle task
pub fn init_with_current(current_thread: Box<Thread>) {
    let mut scheduler_lock = SCHEDULER.lock();
    let thread_id = current_thread.id();

    // Create scheduler with current thread as both idle and current
    let mut scheduler = Scheduler::new(current_thread);
    scheduler.cpu_state[0].current_thread = Some(thread_id);

    *scheduler_lock = Some(scheduler);
    // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
    #[cfg(target_arch = "x86_64")]
    log_serial_println!("Scheduler initialized with current thread {} as idle task", thread_id);
    #[cfg(not(target_arch = "x86_64"))]
    let _ = thread_id;
}

/// Register an idle thread for a secondary CPU.
/// Called during SMP bringup from secondary_cpu_entry_rust.
#[cfg(target_arch = "aarch64")]
pub fn register_cpu_idle_thread(cpu_id: usize, idle_thread: Box<Thread>) {
    without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            scheduler.register_idle_thread(cpu_id, idle_thread);
        }
    });
}

/// Add a thread to the scheduler
pub fn spawn(thread: Box<Thread>) {
    // Disable interrupts to prevent timer interrupt deadlock
    without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            scheduler.add_thread(thread);
            // Ensure a switch happens ASAP (especially in CI smoke runs)
            NEED_RESCHED.store(true, Ordering::Relaxed);
            // Mirror to per-CPU flag so IRQ-exit path sees it
            #[cfg(target_arch = "x86_64")]
            crate::per_cpu::set_need_resched(true);
            #[cfg(target_arch = "aarch64")]
            crate::per_cpu_aarch64::set_need_resched(true);
        } else {
            panic!("Scheduler not initialized");
        }
    });
}

/// Add a thread as the current running thread without scheduling.
///
/// Used when manually starting the first userspace thread (init process).
/// The thread is added to the scheduler's thread list and marked as current,
/// but NOT added to the ready queue and need_resched is NOT set.
/// This allows the thread to run without the scheduler trying to preempt it.
#[allow(dead_code)]
pub fn spawn_as_current(thread: Box<Thread>) {
    without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            scheduler.add_thread_as_current(thread);
            // NOTE: Do NOT set need_resched - we want this thread to run
        } else {
            panic!("Scheduler not initialized");
        }
    });
}

/// Perform scheduling and return threads to switch between
pub fn schedule() -> Option<(u64, u64)> {
    // Check if interrupts are already disabled (i.e., we're in interrupt context)
    let interrupts_were_enabled = are_enabled();

    let result = if interrupts_were_enabled {
        // Normal case: disable interrupts to prevent deadlock
        without_interrupts(|| {
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
#[allow(dead_code)]
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

/// Check if the current thread is the idle thread (safe to call from IRQ context)
/// Returns None if the scheduler lock can't be acquired (to avoid deadlock)
#[allow(dead_code)]
pub fn is_current_idle_thread() -> Option<bool> {
    // Try to get the lock without blocking - if we can't, assume not idle
    // to be safe. This prevents deadlock when timer fires during scheduler ops.
    if let Some(scheduler_lock) = SCHEDULER.try_lock() {
        if let Some(scheduler) = scheduler_lock.as_ref() {
            return Some(
                scheduler
                    .current_thread_id_inner()
                    .map(|id| id == scheduler.idle_thread_id())
                    .unwrap_or(false),
            );
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
    without_interrupts(|| {
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
    without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        scheduler_lock
            .as_mut()
            .and_then(|sched| sched.get_thread_mut(thread_id).map(f))
    })
}

/// Get the current thread ID
/// This function disables interrupts to prevent deadlock with timer interrupt
pub fn current_thread_id() -> Option<u64> {
    without_interrupts(|| {
        let scheduler_lock = SCHEDULER.lock();
        scheduler_lock.as_ref().and_then(|s| s.cpu_state[Scheduler::current_cpu_id()].current_thread)
    })
}

/// Set the current thread ID
/// Used during boot to establish the initial userspace thread as current
/// before jumping to userspace.
#[allow(dead_code)]
pub fn set_current_thread(thread_id: u64) {
    without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            scheduler.set_current_thread(thread_id);
        }
    });
}

/// Yield the current thread
pub fn yield_current() {
    // CRITICAL FIX: Do NOT call schedule() here!
    // schedule() updates self.cpu_state[Self::current_cpu_id()].current_thread, but no actual context switch happens.
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
// self.cpu_state[Self::current_cpu_id()].current_thread. Calling it "just to peek" would corrupt scheduler state.
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
    #[cfg(target_arch = "x86_64")]
    crate::per_cpu::set_need_resched(true);
    #[cfg(target_arch = "aarch64")]
    crate::per_cpu_aarch64::set_need_resched(true);
}

/// Check and clear the need_resched flag (called from interrupt return path)
pub fn check_and_clear_need_resched() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        let need = crate::per_cpu::need_resched();
        if need { crate::per_cpu::set_need_resched(false); }
        let _ = NEED_RESCHED.swap(false, Ordering::Relaxed);
        need
    }
    #[cfg(target_arch = "aarch64")]
    {
        // ARM64: Check per-CPU flag and global atomic
        let need = crate::per_cpu_aarch64::need_resched();
        if need {
            crate::per_cpu_aarch64::set_need_resched(false);
        }
        let _ = NEED_RESCHED.swap(false, Ordering::Relaxed);
        need
    }
}

/// Check if the need_resched flag is set (without clearing it)
/// Used by can_schedule() to determine if kernel threads should be rescheduled
pub fn is_need_resched() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        crate::per_cpu::need_resched() || NEED_RESCHED.load(Ordering::Relaxed)
    }
    #[cfg(target_arch = "aarch64")]
    {
        // ARM64: Check per-CPU flag and global atomic
        crate::per_cpu_aarch64::need_resched() || NEED_RESCHED.load(Ordering::Relaxed)
    }
}

/// Switch to idle thread immediately (for use by exception handlers)
/// This updates scheduler state so subsequent timer interrupts can properly schedule.
/// Call this before modifying exception frame to return to idle_loop.
pub fn switch_to_idle() {
    with_scheduler(|sched| {
        let cpu_id = Scheduler::current_cpu_id();
        let idle_id = sched.cpu_state[cpu_id].idle_thread;
        sched.cpu_state[cpu_id].current_thread = Some(idle_id);

        // Also update per-CPU current thread pointer
        #[cfg(target_arch = "x86_64")]
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

        #[cfg(target_arch = "x86_64")]
        log::info!("Exception handler: Switched scheduler to idle thread {}", idle_id);
    });
}

/// Best-effort switch to idle — uses try_lock to avoid deadlock in crash handlers.
///
/// When an INSTRUCTION_ABORT or DATA_ABORT occurs from EL1, the SCHEDULER lock
/// may already be held (e.g., the crash happened during a context switch). Using
/// `switch_to_idle()` would deadlock on the same CPU. This version uses try_lock:
/// if the lock is available, update scheduler state; if not, just return — the
/// next timer interrupt on this CPU will see the idle loop and correct the state.
#[cfg(target_arch = "aarch64")]
pub fn switch_to_idle_best_effort() {
    if let Some(mut scheduler_lock) = SCHEDULER.try_lock() {
        if let Some(sched) = scheduler_lock.as_mut() {
            let cpu_id = Scheduler::current_cpu_id();
            let idle_id = sched.cpu_state[cpu_id].idle_thread;
            sched.cpu_state[cpu_id].current_thread = Some(idle_id);
        }
    }
    // If try_lock fails, the scheduler state will be stale, but the CPU
    // will be executing idle_loop_arm64 which only does WFI. The next
    // timer-driven schedule() call will see the idle thread running and
    // correct the state.
}

/// Test module for scheduler state invariants
/// These tests use x86_64-specific types (VirtAddr) and are only compiled for x86_64
#[cfg(all(test, target_arch = "x86_64"))]
pub mod tests {
    use super::*;
    use alloc::boxed::Box;
    use alloc::string::String;
    use crate::task::thread::{Thread, ThreadPrivilege, ThreadState};
    use x86_64::VirtAddr;

    fn dummy_entry() {}

    fn make_thread(id: u64, state: ThreadState) -> Box<Thread> {
        let mut thread = Thread::new_with_id(
            id,
            String::from("scheduler-test-thread"),
            dummy_entry,
            VirtAddr::new(0x2000),
            VirtAddr::new(0x1000),
            VirtAddr::new(0),
            ThreadPrivilege::Kernel,
        );
        thread.state = state;
        Box::new(thread)
    }

    pub fn test_unblock_does_not_duplicate_ready_queue() {
        log::info!("=== TEST: unblock avoids duplicate ready_queue entries ===");

        let idle_thread = make_thread(1, ThreadState::Ready);
        let mut scheduler = Scheduler::new(idle_thread);

        let blocked_thread_id = 2;
        let blocked_thread = make_thread(blocked_thread_id, ThreadState::Blocked);
        scheduler.add_thread(blocked_thread);
        if let Some(thread) = scheduler.get_thread_mut(blocked_thread_id) {
            thread.state = ThreadState::Blocked;
        }
        scheduler.remove_from_ready_queue(blocked_thread_id);

        scheduler.unblock(blocked_thread_id);
        scheduler.unblock(blocked_thread_id);

        let count = scheduler
            .ready_queue
            .iter()
            .filter(|&&id| id == blocked_thread_id)
            .count();
        assert_eq!(count, 1);

        log::info!("=== TEST PASSED: unblock avoids duplicate ready_queue entries ===");
    }

    pub fn test_schedule_does_not_duplicate_ready_queue() {
        log::info!("=== TEST: schedule avoids duplicate ready_queue entries ===");

        let idle_thread = make_thread(1, ThreadState::Ready);
        let mut scheduler = Scheduler::new(idle_thread);

        let current_thread_id = 2;
        let current_thread = make_thread(current_thread_id, ThreadState::Running);
        scheduler.add_thread(current_thread);

        let other_thread_id = 3;
        let other_thread = make_thread(other_thread_id, ThreadState::Ready);
        scheduler.add_thread(other_thread);

        scheduler.cpu_state[0].current_thread = Some(current_thread_id);
        if let Some(thread) = scheduler.get_thread_mut(current_thread_id) {
            thread.state = ThreadState::Running;
        }

        let scheduled = scheduler.schedule();
        assert_eq!(scheduled.is_some(), true);

        let count = scheduler
            .ready_queue
            .iter()
            .filter(|&&id| id == current_thread_id)
            .count();
        assert_eq!(count, 1);

        log::info!("=== TEST PASSED: schedule avoids duplicate ready_queue entries ===");
    }

    /// Test that yield_current() does NOT modify scheduler.current_thread.
    ///
    /// This test validates the fix for the bug where yield_current() called schedule(),
    /// which updated self.cpu_state[Self::current_cpu_id()].current_thread without an actual context switch occurring.
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
/// Only available on x86_64 since tests use architecture-specific types.
#[cfg(target_arch = "x86_64")]
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
