//! Thread spawning functionality
//!
//! This module provides the ability to create new kernel threads.

use super::thread::Thread;
use alloc::{boxed::Box, string::ToString};
use x86_64::VirtAddr;

/// Default stack size for threads (64 KB)
const DEFAULT_STACK_SIZE: usize = 64 * 1024;

/// Spawn a new kernel thread
pub fn spawn_thread(name: &str, entry_point: fn()) -> Result<u64, &'static str> {
    // Allocate a stack for the thread
    let stack = crate::memory::stack::allocate_stack(DEFAULT_STACK_SIZE)?;
    
    // Allocate TLS for the thread
    let thread_id = crate::tls::allocate_thread_tls()
        .map_err(|_| "Failed to allocate thread TLS")?;
    
    // Get TLS block address
    let tls_block = crate::tls::get_thread_tls_block(thread_id)
        .ok_or("Failed to get TLS block")?;
    
    // Create the thread
    let thread = Box::new(Thread::new(
        name.to_string(),
        entry_point,
        stack.top(),
        stack.bottom(),
        tls_block,
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
    
    let mut thread = Box::new(Thread::new(
        "idle".to_string(),
        idle_thread_fn,
        VirtAddr::new(0), // Will be set to current RSP
        VirtAddr::new(0), // Will be set appropriately
        VirtAddr::new(crate::tls::current_tls_base()),
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
    }
}

/// Initialize the threading subsystem
pub fn init() {
    // Create and set up the idle thread
    let idle_thread = create_idle_thread();
    
    // Initialize the scheduler with the idle thread
    super::scheduler::init(idle_thread);
    
    log::info!("Threading subsystem initialized");
}