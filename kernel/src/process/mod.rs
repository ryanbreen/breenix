//! Process management for Breenix
//! 
//! This module handles process creation, scheduling, and lifecycle management.
//! A process is a running instance of a program with its own address space.

use alloc::string::String;
use spin::Mutex;

pub mod process;
pub mod manager;
pub mod exec;
pub mod spawn;

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
pub fn manager() -> spin::MutexGuard<'static, Option<ProcessManager>> {
    PROCESS_MANAGER.lock()
}

/// Try to get the process manager without blocking (for interrupt contexts)
pub fn try_manager() -> Option<spin::MutexGuard<'static, Option<ProcessManager>>> {
    PROCESS_MANAGER.try_lock()
}

/// Create and spawn a new process from an ELF binary
pub fn spawn_process(name: String, elf_data: &[u8]) -> Result<ProcessId, &'static str> {
    let mut manager_lock = PROCESS_MANAGER.lock();
    let manager = manager_lock.as_mut().ok_or("Process manager not initialized")?;
    
    manager.create_process(name, elf_data)
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