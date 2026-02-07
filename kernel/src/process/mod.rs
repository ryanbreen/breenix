//! Process management for Breenix
//!
//! This module handles process creation, scheduling, and lifecycle management.
//! A process is a running instance of a program with its own address space.

use spin::Mutex;

pub mod creation;
pub mod fork;
pub mod manager;
pub mod process;

pub use manager::ProcessManager;
pub use process::{Process, ProcessId, ProcessState};

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

impl Drop for ProcessManagerGuard {
    fn drop(&mut self) {
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
        // Save current interrupt state and disable interrupts
        let saved_daif: u64;
        unsafe {
            core::arch::asm!("mrs {}, daif", out(reg) saved_daif, options(nomem, nostack));
            core::arch::asm!("msr daifset, #0xf", options(nomem, nostack));
        }
        let guard = PROCESS_MANAGER.lock();
        ProcessManagerGuard {
            _guard: core::mem::ManuallyDrop::new(guard),
            saved_daif,
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let guard = PROCESS_MANAGER.lock();
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
        manager_lock.as_mut().map(f)
    })
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
        manager_lock.as_mut().map(f)
    })
}

/// Try to get the process manager without blocking (for interrupt contexts)
pub fn try_manager() -> Option<spin::MutexGuard<'static, Option<ProcessManager>>> {
    PROCESS_MANAGER.try_lock()
}

/// Create a new user process using the new architecture
/// Note: Uses architecture-specific ELF loader and process creation
#[allow(dead_code)]
pub fn create_user_process(name: alloc::string::String, elf_data: &[u8]) -> Result<ProcessId, &'static str> {
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
        if let Some(ref mut manager) = *manager() {
            manager.exit_process(pid, exit_code);
        } else {
            log::error!("Process manager not available!");
        }
    } else {
        log::error!("No current PID set!");
    }
}
