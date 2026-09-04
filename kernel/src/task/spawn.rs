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
#[cfg(target_arch = "aarch64")]
use crate::memory::arch_stub::VirtAddr;
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;

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
    let stack = crate::memory::stack::allocate_stack_with_privilege(
        DEFAULT_STACK_SIZE,
        to_stack_privilege(privilege),
    )?;

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

    // Mark idle thread as already running. The id stays the one `Thread::new`
    // allocated: 0 is the no-thread sentinel and is never a live thread id.
    thread.state = super::thread::ThreadState::Running;

    thread
}

/// Idle thread function - runs when nothing else is ready
#[allow(dead_code)]
fn idle_thread_fn() {
    loop {
        // #772: this loop parks on a raw `enable_and_hlt`/`wfi` rather than on
        // `crate::arch_halt_with_interrupts`, so the park count the revisit
        // oracle reads is bumped here by hand -- once, covering both arches.
        // This is the idle thread, so what it bumps is the idle thread's own
        // count; the one reader compares a thread's count against a stamp
        // taken from the same thread, so an idle park cannot perturb another
        // thread's reading. `crate::arch_halt` says the same at more length.
        crate::per_cpu::note_wait_loop_park();

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
                    "msr daifclr, #3", // Clear IRQ+FIQ mask (enable interrupts)
                    "wfi",             // Wait For Interrupt
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

/// Initialize the threading subsystem
#[allow(dead_code)]
pub fn init() {
    // Create and set up the idle thread
    let idle_thread = create_idle_thread();

    // Initialize the scheduler with the idle thread
    super::scheduler::init(idle_thread);

    log::info!("Threading subsystem initialized");
}
