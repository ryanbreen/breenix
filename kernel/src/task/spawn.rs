//! Thread spawning functionality
//!
//! This module provides the ability to create new kernel threads.

use super::thread::{Thread, ThreadPrivilege};
use alloc::{boxed::Box, string::ToString};
use x86_64::VirtAddr;
use crate::elf;

/// Default stack size for threads (64 KB)
const DEFAULT_STACK_SIZE: usize = 64 * 1024;

/// Spawn a new kernel thread
pub fn spawn_thread(name: &str, entry_point: fn()) -> Result<u64, &'static str> {
    spawn_thread_with_privilege(name, entry_point, ThreadPrivilege::Kernel)
}

/// Spawn a new thread with specified privilege
pub fn spawn_thread_with_privilege(
    name: &str, 
    entry_point: fn(), 
    privilege: ThreadPrivilege
) -> Result<u64, &'static str> {
    // Allocate a stack for the thread with appropriate privilege
    let stack = crate::memory::stack::allocate_stack_with_privilege(DEFAULT_STACK_SIZE, privilege)?;
    
    // Allocate TLS for the thread
    let thread_id = crate::tls::allocate_thread_tls()
        .map_err(|_| "Failed to allocate thread TLS")?;
    
    // Get TLS block address
    let tls_block = crate::tls::get_thread_tls_block(thread_id)
        .ok_or("Failed to get TLS block")?;
    
    // Create the thread with specified privilege
    let thread = Box::new(Thread::new(
        name.to_string(),
        entry_point,
        stack.top(),
        stack.bottom(),
        tls_block,
        privilege,
    ));
    
    let tid = thread.id();
    
    // Add to scheduler
    super::scheduler::spawn(thread);
    
    log::info!("Spawned thread '{}' with ID {}", name, tid);
    
    Ok(tid)
}

/// Create the idle thread
/// The idle thread runs when no other threads are ready
pub fn create_idle_thread() -> Box<Thread> {
    // Idle thread uses the current stack and TLS (kernel main thread)
    // It doesn't need its own stack since it's already running
    
    let tls_base = crate::tls::current_tls_base();
    
    let mut thread = Box::new(Thread::new(
        "idle".to_string(),
        idle_thread_fn,
        VirtAddr::new(0), // Will be set to current RSP
        VirtAddr::new(0), // Will be set appropriately
        VirtAddr::new(tls_base),
        ThreadPrivilege::Kernel,
    ));
    
    // Mark idle thread as already running
    thread.state = super::thread::ThreadState::Running;
    thread.id = 0; // Kernel thread has ID 0
    
    thread
}

/// Idle thread function - runs when nothing else is ready
fn idle_thread_fn() {
    loop {
        // Enable interrupts and halt until next interrupt
        x86_64::instructions::interrupts::enable_and_hlt();
        
        // Check if there are any ready threads
        if let Some(has_work) = super::scheduler::with_scheduler(|s| s.has_runnable_threads()) {
            if has_work {
                // Yield to let scheduler pick a ready thread
                super::scheduler::yield_current();
            }
        }
        
        // Periodically wake keyboard task to ensure responsiveness
        // This helps when returning from userspace execution
        static mut WAKE_COUNTER: u64 = 0;
        unsafe {
            WAKE_COUNTER += 1;
            if WAKE_COUNTER % 100 == 0 {
                crate::keyboard::stream::wake_keyboard_task();
            }
        }
    }
}

/// Spawn a userspace thread from ELF binary data
pub fn spawn_userspace_from_elf(name: &str, elf_data: &[u8]) -> Result<u64, &'static str> {
    // Load the ELF binary
    let loaded_elf = elf::load_elf(elf_data)?;
    
    // Allocate user stack (128KB)
    const USER_STACK_SIZE: usize = 128 * 1024;
    let stack = crate::memory::stack::allocate_stack_with_privilege(
        USER_STACK_SIZE, 
        ThreadPrivilege::User
    )?;
    let stack_top = stack.top();
    
    // Keep the stack alive by storing it somewhere
    // For now, we'll leak it - in a real implementation, the thread would own it
    let _stack = Box::leak(Box::new(stack));
    
    log::debug!("Allocating TLS for thread");
    // Allocate TLS for the thread
    let thread_id = match crate::tls::allocate_thread_tls() {
        Ok(id) => {
            log::debug!("TLS allocated successfully, thread ID: {}", id);
            id
        }
        Err(e) => {
            log::error!("Failed to allocate TLS: {}", e);
            return Err("Failed to allocate thread TLS");
        }
    };
    
    log::debug!("Getting TLS block address for thread {}", thread_id);
    // Get TLS block address
    let tls_block = crate::tls::get_thread_tls_block(thread_id)
        .ok_or("Failed to get TLS block")?;
    
    log::debug!("TLS block at {:?}", tls_block);
    
    // Create the userspace thread
    let thread = Box::new(Thread::new_userspace(
        name.to_string(),
        loaded_elf.entry_point,
        stack_top,
        tls_block,
    ));
    
    let tid = thread.id();
    
    log::debug!("Adding thread to scheduler");
    // Add to scheduler
    super::scheduler::spawn(thread);
    
    log::info!("Spawned userspace thread '{}' with ID {} at entry {:#x}", 
        name, tid, loaded_elf.entry_point);
    
    log::debug!("Yielding to scheduler");
    // Force a scheduler yield to give the new thread a chance to run
    super::scheduler::yield_current();
    
    log::debug!("Returned from yield");
    
    Ok(tid)
}

/// Initialize the threading subsystem
pub fn init() {
    // Create and set up the idle thread
    let idle_thread = create_idle_thread();
    
    // Initialize the scheduler with the idle thread
    super::scheduler::init(idle_thread);
    
    log::info!("Threading subsystem initialized");
}