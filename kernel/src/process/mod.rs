//! Process management for Breenix
//!
//! This module handles process creation, scheduling, and lifecycle management.
//! A process is a running instance of a program with its own address space.

#[cfg(target_arch = "x86_64")]
use spin::Mutex;

#[cfg(target_arch = "x86_64")]
pub mod creation;
#[cfg(target_arch = "x86_64")]
pub mod fork;
#[cfg(target_arch = "x86_64")]
pub mod manager;
pub mod process;

#[cfg(target_arch = "x86_64")]
pub use manager::ProcessManager;
pub use process::{Process, ProcessId, ProcessState};

// x86_64-specific process manager infrastructure
#[cfg(target_arch = "x86_64")]
mod x86_64_manager {
    use super::*;

    /// Wrapper to log when process manager lock is dropped
    pub struct ProcessManagerGuard {
        pub(crate) _guard: spin::MutexGuard<'static, Option<ProcessManager>>,
    }

    impl Drop for ProcessManagerGuard {
        fn drop(&mut self) {
            // Lock release logging removed - too verbose for production
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

    /// Get a reference to the global process manager
    /// NOTE: This acquires a lock without disabling interrupts.
    /// For operations that could be called while holding scheduler locks,
    /// use with_process_manager() instead.
    pub fn manager() -> ProcessManagerGuard {
        let guard = PROCESS_MANAGER.lock();
        ProcessManagerGuard { _guard: guard }
    }

    /// Execute a function with the process manager while interrupts are disabled
    /// This prevents deadlock when the timer interrupt tries to access the process manager
    pub fn with_process_manager<F, R>(f: F) -> Option<R>
    where
        F: FnOnce(&mut ProcessManager) -> R,
    {
        x86_64::instructions::interrupts::without_interrupts(|| {
            let mut manager_lock = PROCESS_MANAGER.lock();
            manager_lock.as_mut().map(f)
        })
    }

    /// Try to get the process manager without blocking (for interrupt contexts)
    pub fn try_manager() -> Option<spin::MutexGuard<'static, Option<ProcessManager>>> {
        PROCESS_MANAGER.try_lock()
    }

    /// Create a new user process using the new architecture
    #[allow(dead_code)]
    pub fn create_user_process(name: alloc::string::String, elf_data: &[u8]) -> Result<ProcessId, &'static str> {
        super::creation::create_user_process(name, elf_data)
    }

    /// Get the current process ID
    #[allow(dead_code)]
    pub fn current_pid() -> Option<ProcessId> {
        let manager_lock = PROCESS_MANAGER.lock();
        let manager = manager_lock.as_ref()?;
        manager.current_pid()
    }

    /// Exit the current process
    #[allow(dead_code)]
    pub fn exit_current(exit_code: i32) {
        log::debug!("exit_current called with code {}", exit_code);

        if let Some(pid) = current_pid() {
            log::debug!("Current PID is {}", pid.as_u64());
            if let Some(ref mut manager) = *PROCESS_MANAGER.lock() {
                manager.exit_process(pid, exit_code);
            } else {
                log::error!("Process manager not available!");
            }
        } else {
            log::error!("No current PID set!");
        }
    }
}

// Re-export x86_64-specific items (some unused but kept for public API)
#[cfg(target_arch = "x86_64")]
#[allow(unused_imports)]
pub use x86_64_manager::{
    init, manager, with_process_manager, try_manager, create_user_process,
    current_pid, exit_current, ProcessManagerGuard, PROCESS_MANAGER,
};

// ARM64 stubs - process manager not yet implemented
#[cfg(target_arch = "aarch64")]
pub fn init() {
    log::info!("Process management stub initialized (ARM64)");
}
