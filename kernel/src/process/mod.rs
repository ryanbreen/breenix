//! Process management for Breenix
//!
//! This module handles process creation, scheduling, and lifecycle management.
//! A process is a running instance of a program with its own address space.

use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

pub mod creation;
pub mod fork;
pub mod manager;
pub mod process;

pub use manager::ProcessManager;
pub use manager::{InitDesignationTicket, InitPublication, FIRST_ORDINARY_PID, RESERVED_INIT_PID};
pub use process::{Process, ProcessId, ProcessState};

/// Result of entering process teardown through the receipt-custody wrapper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitOutcome {
    Missing,
    FirstCommit,
    RepeatCommit,
}

/// Crate-private custody object for a deferred process root. There is no
/// public constructor and no public API that can hand one to a caller.
pub(crate) struct RetirementReceipt {
    reclaim: Option<crate::task::process_task::PendingProcessReclaim>,
}

impl RetirementReceipt {
    pub(crate) fn from_reclaim(reclaim: crate::task::process_task::PendingProcessReclaim) -> Self {
        Self {
            reclaim: Some(reclaim),
        }
    }

    pub(crate) fn take_contents(
        &mut self,
    ) -> Option<crate::task::process_task::PendingProcessReclaim> {
        self.reclaim.take()
    }
}

impl Drop for RetirementReceipt {
    fn drop(&mut self) {
        if let Some(reclaim) = self.take_contents() {
            crate::trace_count!(crate::tracing::providers::teardown::RECEIPT_DROPPED_UNRETIRED);
            crate::task::process_task::enqueue_process_reclaim(reclaim);
        }
    }
}

pub(crate) const PM_LOCK_OWNER_NONE: u64 = u64::MAX;
#[cfg(target_arch = "x86_64")]
pub(crate) const PM_LOCK_OWNER_TID_UNKNOWN: u64 = u64::MAX - 1;

/// Best-effort owner snapshot for PROCESS_MANAGER lock contention analysis.
static PROCESS_MANAGER_OWNER_CPU: AtomicU64 = AtomicU64::new(PM_LOCK_OWNER_NONE);
static PROCESS_MANAGER_OWNER_TID: AtomicU64 = AtomicU64::new(PM_LOCK_OWNER_NONE);

/// Wrapper that holds the process manager lock and restores interrupt state on drop.
///
/// On ARM64, acquiring PROCESS_MANAGER must disable interrupts to prevent
/// a single-CPU deadlock: if a timer interrupt fires while this lock is held,
/// the exception return path calls `set_next_ttbr0_for_thread()` → `manager()`
/// which would spin forever waiting for the lock we already hold.
///
/// Drop order is critical: release the lock FIRST, then restore interrupts.
/// We use ManuallyDrop to control this ordering explicitly.
pub struct ProcessManagerGuard {
    pub(crate) _guard: core::mem::ManuallyDrop<spin::MutexGuard<'static, Option<ProcessManager>>>,
    /// Saved DAIF register value (ARM64 only) - restored on drop to re-enable interrupts
    #[cfg(target_arch = "aarch64")]
    saved_daif: u64,
}

/// Non-blocking process-manager guard with the same owner instrumentation as
/// `manager()`, and the same local-interrupt masking: the two acquisition modes
/// differ only in whether they block (#812).
///
/// The saved state is the interrupt-enable state the acquisition found, and it
/// is restored only after the mutex has been released, so the lock is out of
/// this CPU's hands before an interrupt can be taken on it again.
pub struct TryProcessManagerGuard {
    guard: core::mem::ManuallyDrop<spin::MutexGuard<'static, Option<ProcessManager>>>,
    /// Saved DAIF register value (ARM64 only) - restored on drop.
    #[cfg(target_arch = "aarch64")]
    saved_daif: u64,
    /// Saved RFLAGS.IF state (x86_64 only) - restored on drop.
    #[cfg(target_arch = "x86_64")]
    interrupts_were_enabled: bool,
}

impl Drop for TryProcessManagerGuard {
    fn drop(&mut self) {
        note_process_manager_lock_released();

        // Same ordering rule as ProcessManagerGuard::drop: release the lock
        // BEFORE restoring interrupts. Restoring first would re-open exactly
        // the window this guard exists to close.
        unsafe {
            core::mem::ManuallyDrop::drop(&mut self.guard);
        }

        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!(
                "msr daif, {}",
                in(reg) self.saved_daif,
                options(nomem, nostack)
            );
        }

        #[cfg(target_arch = "x86_64")]
        if self.interrupts_were_enabled {
            unsafe {
                crate::arch_enable_interrupts();
            }
        }
    }
}

impl core::ops::Deref for TryProcessManagerGuard {
    type Target = Option<ProcessManager>;

    fn deref(&self) -> &Self::Target {
        &*self.guard
    }
}

impl core::ops::DerefMut for TryProcessManagerGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self.guard
    }
}

impl Drop for ProcessManagerGuard {
    fn drop(&mut self) {
        // Clear owner metadata before the mutex becomes available to another CPU.
        note_process_manager_lock_released();

        // CRITICAL: Release the lock BEFORE restoring interrupts.
        // If we restored DAIF first, there'd be a window where interrupts are enabled
        // but the lock is still held, allowing the exact deadlock we're preventing.
        unsafe {
            core::mem::ManuallyDrop::drop(&mut self._guard);
        }

        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!(
                "msr daif, {}",
                in(reg) self.saved_daif,
                options(nomem, nostack)
            );
        }
    }
}

impl core::ops::Deref for ProcessManagerGuard {
    type Target = Option<ProcessManager>;

    fn deref(&self) -> &Self::Target {
        &*self._guard
    }
}

impl core::ops::DerefMut for ProcessManagerGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self._guard
    }
}

/// Global process manager
pub static PROCESS_MANAGER: Mutex<Option<ProcessManager>> = Mutex::new(None);

#[inline(always)]
fn note_process_manager_lock_acquired() {
    let (cpu, tid) = current_process_manager_owner_identity();
    PROCESS_MANAGER_OWNER_TID.store(tid, Ordering::Relaxed);
    PROCESS_MANAGER_OWNER_CPU.store(cpu, Ordering::Release);
    crate::tracing::providers::teardown::note_process_manager_acquire();
}

#[inline(always)]
fn note_process_manager_lock_released() {
    PROCESS_MANAGER_OWNER_TID.store(PM_LOCK_OWNER_NONE, Ordering::Relaxed);
    PROCESS_MANAGER_OWNER_CPU.store(PM_LOCK_OWNER_NONE, Ordering::Release);
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn current_process_manager_owner_identity() -> (u64, u64) {
    let cpu = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id();
    (cpu, 0)
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn current_process_manager_owner_identity() -> (u64, u64) {
    use crate::arch_impl::current::percpu::X86PerCpu;
    use crate::arch_impl::PerCpuOps;

    let tid = crate::per_cpu::current_thread_id_lock_free().unwrap_or(PM_LOCK_OWNER_TID_UNKNOWN);
    (X86PerCpu::cpu_id(), tid)
}

/// Snapshot the best-effort PROCESS_MANAGER lock owner.
pub fn process_manager_owner_snapshot() -> Option<(u64, u64)> {
    let cpu = PROCESS_MANAGER_OWNER_CPU.load(Ordering::Acquire);
    if cpu == PM_LOCK_OWNER_NONE {
        return None;
    }
    let tid = PROCESS_MANAGER_OWNER_TID.load(Ordering::Relaxed);
    Some((cpu, tid))
}

/// Whether this CPU currently owns the process-manager lock.
#[inline(always)]
pub fn process_manager_held_on_current_cpu() -> bool {
    process_manager_owner_snapshot()
        .is_some_and(|(cpu, _)| cpu == current_process_manager_owner_identity().0)
}

/// Initialize the process management system
pub fn init() {
    let manager = ProcessManager::new();
    *PROCESS_MANAGER.lock() = Some(manager);
    log::info!("Process management initialized");
}

/// Get a reference to the global process manager.
///
/// On ARM64, this disables interrupts before acquiring the lock to prevent
/// single-CPU deadlocks where a timer interrupt tries to re-acquire the lock
/// from the context switch path.
pub fn manager() -> ProcessManagerGuard {
    #[cfg(target_arch = "aarch64")]
    {
        let saved_daif: u64;
        unsafe {
            core::arch::asm!("mrs {}, daif", out(reg) saved_daif, options(nomem, nostack));
            core::arch::asm!("msr daifset, #0xf", options(nomem, nostack));
        }
        let guard = PROCESS_MANAGER.lock();
        note_process_manager_lock_acquired();
        ProcessManagerGuard {
            _guard: core::mem::ManuallyDrop::new(guard),
            saved_daif,
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let guard = PROCESS_MANAGER.lock();
        note_process_manager_lock_acquired();
        ProcessManagerGuard {
            _guard: core::mem::ManuallyDrop::new(guard),
        }
    }
}

/// Execute a function with the process manager while interrupts are disabled
/// This prevents deadlock when the timer interrupt tries to access the process manager
#[cfg(target_arch = "x86_64")]
pub fn with_process_manager<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut ProcessManager) -> R,
{
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut manager_lock = PROCESS_MANAGER.lock();
        note_process_manager_lock_acquired();
        let result = manager_lock.as_mut().map(f);
        note_process_manager_lock_released();
        drop(manager_lock);
        result
    })
}

/// The only public process-exit entry point. The locked half can return a
/// receipt only into this function; PM is out of scope before the receipt is
/// enqueued and before notification redemption enters scheduler/SERIAL code.
pub fn exit_process_and_retire(pid: ProcessId, exit_code: i32) -> ExitOutcome {
    let locked = with_process_manager(|pm| {
        let Some(process) = pm.get_process(pid) else {
            return (ExitOutcome::Missing, None, None, exit_code);
        };
        let outcome = if process.is_terminated() {
            ExitOutcome::RepeatCommit
        } else {
            ExitOutcome::FirstCommit
        };
        let thread_id = process.main_thread.as_ref().map(|thread| thread.id);
        let receipt = pm.exit_process_locked(pid, exit_code);
        let reported_exit_code = pm
            .get_process(pid)
            .and_then(|process| process.exit_code)
            .unwrap_or(exit_code);
        (outcome, receipt, thread_id, reported_exit_code)
    });

    let Some((outcome, receipt, thread_id, reported_exit_code)) = locked else {
        return ExitOutcome::Missing;
    };

    if let Some(mut receipt) = receipt {
        if let Some(reclaim) = receipt.take_contents() {
            crate::task::process_task::enqueue_process_reclaim(reclaim);
        }
    }
    #[cfg(target_arch = "x86_64")]
    crate::task::process_task::reclaim_deferred_process_resources();

    // The unchanged btrt effect remains in handle_thread_exit. Calling the
    // existing two-phase path here lets a remote commit race a natural thread
    // exit for the row's report claim without ever invoking SERIAL under PM.
    if let Some(thread_id) = thread_id {
        crate::task::process_task::ProcessScheduler::handle_thread_exit(
            thread_id,
            reported_exit_code,
        );
    }

    outcome
}

/// Execute a function with the process manager while interrupts are disabled (ARM64)
/// This prevents deadlock when timer interrupts try to access the process manager
#[cfg(target_arch = "aarch64")]
pub fn with_process_manager<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut ProcessManager) -> R,
{
    crate::arch_impl::aarch64::cpu::without_interrupts(|| {
        let mut manager_lock = PROCESS_MANAGER.lock();
        note_process_manager_lock_acquired();
        let result = manager_lock.as_mut().map(f);
        note_process_manager_lock_released();
        drop(manager_lock);
        result
    })
}

/// Try to get the process manager without blocking (for interrupt contexts).
///
/// The acquisition masks local interrupts first and the guard restores the
/// saved state after the release, so a holder cannot be interrupted on its own
/// CPU while it owns the lock. A failed `try_lock` restores immediately and
/// leaves interrupt state exactly as it found it.
///
/// #812: without the mask, a thread-context holder here could take an IRQ whose
/// exit path runs the NetRx softirq on the same CPU, and that softirq acquires
/// PROCESS_MANAGER through the blocking `with_process_manager`. `spin::Mutex`
/// is not reentrant, so the CPU waits on a lock it already owns. The mask makes
/// this accessor differ from `manager()` only in whether it blocks.
pub fn try_manager() -> Option<TryProcessManagerGuard> {
    #[cfg(target_arch = "aarch64")]
    {
        let saved_daif: u64;
        unsafe {
            core::arch::asm!("mrs {}, daif", out(reg) saved_daif, options(nomem, nostack));
            core::arch::asm!("msr daifset, #0xf", options(nomem, nostack));
        }
        let Some(guard) = PROCESS_MANAGER.try_lock() else {
            unsafe {
                core::arch::asm!(
                    "msr daif, {}",
                    in(reg) saved_daif,
                    options(nomem, nostack)
                );
            }
            return None;
        };
        note_process_manager_lock_acquired();
        Some(TryProcessManagerGuard {
            guard: core::mem::ManuallyDrop::new(guard),
            saved_daif,
        })
    }
    #[cfg(target_arch = "x86_64")]
    {
        let interrupts_were_enabled = crate::arch_interrupts_enabled();
        unsafe {
            crate::arch_disable_interrupts();
        }
        let Some(guard) = PROCESS_MANAGER.try_lock() else {
            if interrupts_were_enabled {
                unsafe {
                    crate::arch_enable_interrupts();
                }
            }
            return None;
        };
        note_process_manager_lock_acquired();
        Some(TryProcessManagerGuard {
            guard: core::mem::ManuallyDrop::new(guard),
            interrupts_were_enabled,
        })
    }
}

/// Per-process info for lockup diagnostics (small, stack-allocated).
pub struct ProcessDumpEntry {
    pub pid: u64,
    pub name: alloc::string::String,
    pub state_str: alloc::string::String,
}

/// Diagnostic snapshot of process manager state for the soft lockup detector.
pub struct ProcessDumpInfo {
    pub total_processes: u64,
    pub running_count: u64,
    pub blocked_count: u64,
    pub processes: alloc::vec::Vec<ProcessDumpEntry>,
}

/// Try to get a snapshot of process manager state without blocking.
/// Returns None if the lock is held (which is itself diagnostic info).
/// Safe to call from interrupt context.
pub fn try_dump_state() -> Option<ProcessDumpInfo> {
    let guard = PROCESS_MANAGER.try_lock()?;
    let pm = guard.as_ref()?;

    let procs = pm.all_processes();
    let mut running_count = 0u64;
    let mut blocked_count = 0u64;
    let mut entries = alloc::vec::Vec::new();

    for p in &procs {
        let state_str = match p.state {
            ProcessState::Creating => "creating",
            ProcessState::Ready => "ready",
            ProcessState::Running => {
                running_count += 1;
                "running"
            }
            ProcessState::Blocked => {
                blocked_count += 1;
                "blocked"
            }
            ProcessState::Terminated(_) => "terminated",
        };
        entries.push(ProcessDumpEntry {
            pid: p.id.as_u64(),
            name: p.name.clone(),
            state_str: alloc::string::String::from(state_str),
        });
    }

    Some(ProcessDumpInfo {
        total_processes: procs.len() as u64,
        running_count,
        blocked_count,
        processes: entries,
    })
}

/// Create a new user process using the new architecture
/// Note: Uses architecture-specific ELF loader and process creation
#[allow(dead_code)]
pub fn create_user_process(
    name: alloc::string::String,
    elf_data: &[u8],
) -> Result<ProcessId, &'static str> {
    creation::create_user_process(name, elf_data)
}

/// Get the current process ID
#[allow(dead_code)]
pub fn current_pid() -> Option<ProcessId> {
    let manager_guard = manager();
    let manager = manager_guard.as_ref()?;
    manager.current_pid()
}

/// Exit the current process
#[allow(dead_code)]
pub fn exit_current(exit_code: i32) {
    log::debug!("exit_current called with code {}", exit_code);

    if let Some(pid) = current_pid() {
        log::debug!("Current PID is {}", pid.as_u64());
        exit_process_by_pid(pid, exit_code);
    } else {
        log::error!("No current PID set!");
    }
}

fn exit_process_by_pid(pid: ProcessId, exit_code: i32) {
    let _ = exit_process_and_retire(pid, exit_code);
}

#[cfg(feature = "boot_tests")]
pub(crate) fn exit_process_for_teardown_test(pid: ProcessId, exit_code: i32) {
    exit_process_by_pid(pid, exit_code);
}
