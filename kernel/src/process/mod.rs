//! Process management for Breenix
//! 
//! This module handles process creation, scheduling, and lifecycle management.
//! A process is a running instance of a program with its own address space.

use alloc::string::String;
use spin::Mutex;

pub mod process;
pub mod manager;
pub mod fork;
pub mod creation;

pub use process::{Process, ProcessId};
pub use manager::ProcessManager;

/// Wrapper to log when process manager lock is dropped
pub struct ProcessManagerGuard {
    _guard: spin::MutexGuard<'static, Option<ProcessManager>>,
}

impl Drop for ProcessManagerGuard {
    fn drop(&mut self) {
        log::debug!("PROCESS_MANAGER lock released");
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
static PROCESS_MANAGER: Mutex<Option<ProcessManager>> = Mutex::new(None);

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
    log::debug!("Acquiring PROCESS_MANAGER lock");
    let guard = PROCESS_MANAGER.lock();
    log::debug!("PROCESS_MANAGER lock acquired");
    ProcessManagerGuard { _guard: guard }
}

/// Execute a function with the process manager while interrupts are disabled
/// This prevents deadlock when the timer interrupt tries to access the process manager
pub fn with_process_manager<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut ProcessManager) -> R,
{
    x86_64::instructions::interrupts::without_interrupts(|| {
        log::debug!("with_process_manager: Acquiring PROCESS_MANAGER lock (interrupts disabled)");
        let mut manager_lock = PROCESS_MANAGER.lock();
        log::debug!("with_process_manager: PROCESS_MANAGER lock acquired");
        let result = manager_lock.as_mut().map(f);
        log::debug!("with_process_manager: Releasing PROCESS_MANAGER lock");
        result
    })
}

/// Try to get the process manager without blocking (for interrupt contexts)
pub fn try_manager() -> Option<spin::MutexGuard<'static, Option<ProcessManager>>> {
    PROCESS_MANAGER.try_lock()
}

/// Create a new user process using the new architecture
pub fn create_user_process(name: String, elf_data: &[u8]) -> Result<ProcessId, &'static str> {
    creation::create_user_process(name, elf_data)
}

/// Get the current process ID
pub fn current_pid() -> Option<ProcessId> {
    log::trace!("Getting current PID...");
    log::debug!("current_pid: Acquiring PROCESS_MANAGER lock");
    let manager_lock = PROCESS_MANAGER.lock();
    log::debug!("current_pid: PROCESS_MANAGER lock acquired");
    let manager = manager_lock.as_ref()?;
    let pid = manager.current_pid();
    log::trace!("Current PID: {:?}", pid);
    log::debug!("current_pid: Releasing PROCESS_MANAGER lock");
    pid
}

/// Exit the current process
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