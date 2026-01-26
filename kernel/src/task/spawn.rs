//! Thread spawning functionality
//!
//! This module provides the ability to create new kernel threads.
//!
//! This module supports both x86_64 and AArch64 architectures with
//! appropriate cfg guards for architecture-specific functionality.

use super::thread::Thread;
use alloc::{boxed::Box, string::ToString};

// On x86_64, ThreadPrivilege is the same for both thread and stack modules
#[cfg(target_arch = "x86_64")]
use super::thread::ThreadPrivilege;

// On ARM64, there are two ThreadPrivilege types:
// - task::thread::ThreadPrivilege for Thread::new(), CpuContext::new()
// - memory::arch_stub::ThreadPrivilege for memory::stack functions
// We import the thread one as the main type and use the stack one explicitly
#[cfg(target_arch = "aarch64")]
use super::thread::ThreadPrivilege;
#[cfg(target_arch = "aarch64")]
use crate::memory::arch_stub::ThreadPrivilege as StackThreadPrivilege;

// Architecture-specific imports for VirtAddr
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;
#[cfg(target_arch = "aarch64")]
use crate::memory::arch_stub::VirtAddr;

// Architecture-specific ELF loaders
#[cfg(target_arch = "x86_64")]
use crate::elf;
#[cfg(target_arch = "aarch64")]
use crate::arch_impl::aarch64::elf as arm64_elf;

/// Default stack size for threads (64 KB)
#[allow(dead_code)]
const DEFAULT_STACK_SIZE: usize = 64 * 1024;

/// Convert thread::ThreadPrivilege to stack's ThreadPrivilege (ARM64 only)
/// This is needed because memory::stack uses arch_stub::ThreadPrivilege on ARM64
#[cfg(target_arch = "aarch64")]
fn to_stack_privilege(privilege: ThreadPrivilege) -> StackThreadPrivilege {
    match privilege {
        ThreadPrivilege::Kernel => StackThreadPrivilege::Kernel,
        ThreadPrivilege::User => StackThreadPrivilege::User,
    }
}

/// Spawn a new kernel thread
#[allow(dead_code)]
pub fn spawn_thread(name: &str, entry_point: fn()) -> Result<u64, &'static str> {
    spawn_thread_with_privilege(name, entry_point, ThreadPrivilege::Kernel)
}

/// Spawn a new thread with specified privilege
#[allow(dead_code)]
pub fn spawn_thread_with_privilege(
    name: &str,
    entry_point: fn(),
    privilege: ThreadPrivilege,
) -> Result<u64, &'static str> {
    // Allocate a stack for the thread with appropriate privilege
    // On ARM64, we need to convert the ThreadPrivilege type for the stack module
    #[cfg(target_arch = "x86_64")]
    let stack = crate::memory::stack::allocate_stack_with_privilege(DEFAULT_STACK_SIZE, privilege)?;
    #[cfg(target_arch = "aarch64")]
    let stack = crate::memory::stack::allocate_stack_with_privilege(DEFAULT_STACK_SIZE, to_stack_privilege(privilege))?;

    // Allocate TLS for the thread (x86_64 only for now)
    #[cfg(target_arch = "x86_64")]
    let tls_block = {
        let thread_id =
            crate::tls::allocate_thread_tls().map_err(|_| "Failed to allocate thread TLS")?;
        crate::tls::get_thread_tls_block(thread_id).ok_or("Failed to get TLS block")?
    };

    // ARM64: TLS not yet implemented, use placeholder
    #[cfg(target_arch = "aarch64")]
    let tls_block = VirtAddr::new(0);

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
#[allow(dead_code)]
pub fn create_idle_thread() -> Box<Thread> {
    // Idle thread uses the current stack and TLS (kernel main thread)
    // It doesn't need its own stack since it's already running

    // Get TLS base (x86_64 only for now)
    #[cfg(target_arch = "x86_64")]
    let tls_base = crate::tls::current_tls_base();

    // ARM64: TLS not yet implemented
    #[cfg(target_arch = "aarch64")]
    let tls_base = 0u64;

    let mut thread = Box::new(Thread::new(
        "idle".to_string(),
        idle_thread_fn,
        VirtAddr::new(0), // Will be set to current RSP/SP
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
#[allow(dead_code)]
fn idle_thread_fn() {
    loop {
        // Enable interrupts and halt until next interrupt
        // Architecture-specific implementation
        #[cfg(target_arch = "x86_64")]
        {
            x86_64::instructions::interrupts::enable_and_hlt();
        }

        #[cfg(target_arch = "aarch64")]
        {
            // ARM64: enable interrupts and wait for interrupt (WFI)
            // SAFETY: This is the idle thread - we want to halt until an interrupt
            unsafe {
                // Clear DAIF.I to enable IRQs, then wait for interrupt
                core::arch::asm!(
                    "msr daifclr, #2",  // Clear IRQ mask (enable interrupts)
                    "wfi",               // Wait For Interrupt
                    options(nomem, nostack)
                );
            }
        }

        // Check if there are any ready threads
        if let Some(has_work) = super::scheduler::with_scheduler(|s| s.has_runnable_threads()) {
            if has_work {
                // Yield to let scheduler pick a ready thread
                super::scheduler::yield_current();
            }
        }

        // Periodically wake keyboard task to ensure responsiveness
        // This helps when returning from userspace execution
        // Note: keyboard module is x86_64 only
        #[cfg(target_arch = "x86_64")]
        {
            static mut WAKE_COUNTER: u64 = 0;
            unsafe {
                WAKE_COUNTER += 1;
                if WAKE_COUNTER % 100 == 0 {
                    crate::keyboard::stream::wake_keyboard_task();
                }
            }
        }
    }
}

/// Spawn a userspace thread from ELF binary data (x86_64)
///
/// This function loads an ELF binary into memory and creates a userspace
/// thread to execute it. On x86_64, TLS is allocated for the thread.
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
pub fn spawn_userspace_from_elf(name: &str, elf_data: &[u8]) -> Result<u64, &'static str> {
    // Load the ELF binary
    let loaded_elf = elf::load_elf(elf_data)?;

    // Allocate user stack (128KB)
    const USER_STACK_SIZE: usize = 128 * 1024;
    let stack = crate::memory::stack::allocate_stack_with_privilege(
        USER_STACK_SIZE,
        ThreadPrivilege::User,
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
    let tls_block = crate::tls::get_thread_tls_block(thread_id).ok_or("Failed to get TLS block")?;

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

    log::info!(
        "Spawned userspace thread '{}' with ID {} at entry {:#x}",
        name,
        tid,
        loaded_elf.entry_point
    );

    log::debug!("Yielding to scheduler");
    // Force a scheduler yield to give the new thread a chance to run
    super::scheduler::yield_current();

    log::debug!("Returned from yield");

    Ok(tid)
}

/// Spawn a userspace thread from ELF binary data (ARM64)
///
/// On ARM64, userspace ELF loading typically goes through the process manager
/// infrastructure. This function provides a simpler path for testing purposes.
///
/// Note: For full process isolation on ARM64, use the process manager's
/// `create_process()` method instead, which properly sets up page tables.
#[cfg(target_arch = "aarch64")]
#[allow(dead_code)]
pub fn spawn_userspace_from_elf(name: &str, elf_data: &[u8]) -> Result<u64, &'static str> {
    // Validate ELF header first
    let _header = arm64_elf::validate_elf_header(elf_data)?;

    // Allocate user stack (128KB)
    const USER_STACK_SIZE: usize = 128 * 1024;
    let stack = crate::memory::stack::allocate_stack_with_privilege(
        USER_STACK_SIZE,
        to_stack_privilege(ThreadPrivilege::User),
    )?;
    let stack_top = stack.top();

    // Keep the stack alive by storing it somewhere
    // For now, we'll leak it - in a real implementation, the thread would own it
    let _stack = Box::leak(Box::new(stack));

    // For ARM64, load ELF into kernel space for testing
    // SAFETY: We're loading a trusted ELF binary for testing
    let loaded_elf = unsafe { arm64_elf::load_elf_kernel_space(elf_data)? };

    log::debug!("ELF loaded: entry={:#x}, segments_end={:#x}",
        loaded_elf.entry_point, loaded_elf.segments_end);

    // ARM64: TLS not yet implemented, use placeholder
    let tls_block = VirtAddr::new(0);

    // Create the userspace thread
    let thread = Box::new(Thread::new_userspace(
        name.to_string(),
        VirtAddr::new(loaded_elf.entry_point),
        stack_top,
        tls_block,
    ));

    let tid = thread.id();

    log::debug!("Adding thread to scheduler");
    // Add to scheduler
    super::scheduler::spawn(thread);

    log::info!(
        "Spawned userspace thread '{}' with ID {} at entry {:#x}",
        name,
        tid,
        loaded_elf.entry_point
    );

    log::debug!("Yielding to scheduler");
    // Force a scheduler yield to give the new thread a chance to run
    super::scheduler::yield_current();

    log::debug!("Returned from yield");

    Ok(tid)
}

/// Initialize the threading subsystem
#[allow(dead_code)]
pub fn init() {
    // Create and set up the idle thread
    let idle_thread = create_idle_thread();

    // Initialize the scheduler with the idle thread
    super::scheduler::init(idle_thread);

    log::info!("Threading subsystem initialized");
}
