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
pub fn manager() -> spin::MutexGuard<'static, Option<ProcessManager>> {
    PROCESS_MANAGER.lock()
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
pub fn create_user_process(name: String, elf_data: &[u8]) -> Result<ProcessId, &'static str> {
    creation::create_user_process(name, elf_data)
}

/// Get the current process ID
pub fn current_pid() -> Option<ProcessId> {
    log::trace!("Getting current PID...");
    let manager_lock = PROCESS_MANAGER.lock();
    log::trace!("Got manager lock");
    let manager = manager_lock.as_ref()?;
    let pid = manager.current_pid();
    log::trace!("Current PID: {:?}", pid);
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