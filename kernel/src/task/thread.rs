//! Thread management for preemptive multitasking
//!
//! This module implements real threads with preemptive scheduling,
//! building on top of the existing async executor infrastructure.

use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::VirtAddr;

/// Global thread ID counter
static NEXT_THREAD_ID: AtomicU64 = AtomicU64::new(1); // 0 is reserved for kernel thread

/// Allocate a new thread ID
pub fn allocate_thread_id() -> u64 {
    NEXT_THREAD_ID.fetch_add(1, Ordering::SeqCst)
}

/// Thread states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    /// Thread is currently running on CPU
    Running,
    /// Thread is ready to run and in scheduler queue
    Ready,
    /// Thread is blocked waiting for something
    #[allow(dead_code)]
    Blocked,
    /// Thread has terminated
    Terminated,
}

/// Thread privilege level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadPrivilege {
    /// Kernel thread (Ring 0)
    Kernel,
    /// User thread (Ring 3)
    User,
}

/// CPU context saved during context switch
#[derive(Debug, Clone)]
#[repr(C)]
pub struct CpuContext {
    /// General purpose registers
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,

    /// Instruction pointer
    pub rip: u64,

    /// CPU flags
    pub rflags: u64,

    /// Segment registers (for future userspace support)
    pub cs: u64,
    pub ss: u64,
}

impl CpuContext {
    /// Create a new CPU context for a thread entry point
    pub fn new(entry_point: VirtAddr, stack_pointer: VirtAddr, privilege: ThreadPrivilege) -> Self {
        Self {
            // Zero all general purpose registers
            rax: 0,
            rbx: 0,
            rcx: 0,
            rdx: 0,
            rsi: 0,
            rdi: 0,
            rbp: 0,
            rsp: stack_pointer.as_u64(),
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,

            // Set instruction pointer to entry point
            rip: entry_point.as_u64(),

            // Set default flags based on privilege
            // For kernel threads, start with interrupts disabled to prevent
            // immediate preemption before critical initialization
            // CRITICAL: Bit 1 (0x2) must ALWAYS be set in RFLAGS!
            rflags: match privilege {
                ThreadPrivilege::Kernel => 0x002, // No IF flag - interrupts disabled
                ThreadPrivilege::User => 0x202, // IF flag set + mandatory bit 1 - interrupts enabled
            },

            // Segments based on privilege level
            // Note: These values are just placeholders - the actual segment selectors
            // will be set correctly during context restore based on the GDT
            cs: match privilege {
                ThreadPrivilege::Kernel => 0x08, // Kernel code segment
                ThreadPrivilege::User => 0x33,   // User code segment (based on GDT log)
            },
            ss: match privilege {
                ThreadPrivilege::Kernel => 0x10, // Kernel data segment
                ThreadPrivilege::User => 0x2b,   // User data segment (based on GDT log)
            },
        }
    }
}

/// Extended Thread Control Block for preemptive multitasking
pub struct Thread {
    /// Thread ID
    pub id: u64,

    /// Thread name (for debugging)
    pub name: alloc::string::String,

    /// Current state
    pub state: ThreadState,

    /// CPU context (registers)
    pub context: CpuContext,

    /// Stack information
    pub stack_top: VirtAddr,
    pub stack_bottom: VirtAddr,

    /// Kernel stack for syscalls/interrupts (only for userspace threads)
    pub kernel_stack_top: Option<VirtAddr>,

    /// Kernel stack allocation (must be kept alive for RAII)
    #[allow(dead_code)]
    pub kernel_stack_allocation: Option<crate::memory::kernel_stack::KernelStack>,

    /// TLS block address
    pub tls_block: VirtAddr,

    /// Priority (0 = highest)
    pub priority: u8,

    /// Time slice remaining (in timer ticks)
    pub time_slice: u32,

    /// Entry point function
    pub entry_point: Option<fn()>,

    /// Privilege level
    pub privilege: ThreadPrivilege,
    
    /// Has this thread ever run? (false for brand new threads)
    pub has_started: bool,
}

impl Clone for Thread {
    fn clone(&self) -> Self {
        Thread {
            id: self.id,
            name: self.name.clone(),
            state: self.state,
            context: self.context.clone(),
            stack_top: self.stack_top,
            stack_bottom: self.stack_bottom,
            kernel_stack_top: self.kernel_stack_top,
            kernel_stack_allocation: None, // Can't clone kernel stack allocation
            tls_block: self.tls_block,
            priority: self.priority,
            time_slice: self.time_slice,
            entry_point: self.entry_point, // fn pointers can be copied
            privilege: self.privilege,
            has_started: self.has_started,
        }
    }
}

impl Thread {
    /// Create a new kernel thread with an argument
    pub fn new_kernel(
        name: alloc::string::String,
        entry_point: extern "C" fn(u64) -> !,
        arg: u64,
    ) -> Result<Self, &'static str> {
        let id = NEXT_THREAD_ID.fetch_add(1, Ordering::SeqCst);

        // Allocate a kernel stack
        const KERNEL_STACK_SIZE: usize = 16 * 1024; // 16 KiB (ignored by bitmap allocator)
        let stack = crate::memory::alloc_kernel_stack(KERNEL_STACK_SIZE)
            .ok_or("Failed to allocate kernel stack")?;

        let stack_top = stack.top();
        let stack_bottom = stack.bottom();

        // Set up initial context for kernel thread
        let mut context = CpuContext::new(
            VirtAddr::new(entry_point as u64),
            stack_top,
            ThreadPrivilege::Kernel,
        );

        // Pass argument in RDI (System V ABI)
        context.rdi = arg;

        // Kernel threads don't need TLS
        let tls_block = VirtAddr::new(0);

        Ok(Self {
            id,
            name,
            state: ThreadState::Ready,
            context,
            stack_top,
            stack_bottom,
            kernel_stack_top: Some(stack_top), // Kernel threads use their stack for everything
            kernel_stack_allocation: Some(stack), // Keep allocation alive
            tls_block,
            priority: 64,      // Higher priority for kernel threads
            time_slice: 20,    // Longer time slice
            entry_point: None, // Kernel threads use direct entry
            privilege: ThreadPrivilege::Kernel,
            has_started: false, // New thread hasn't run yet
        })
    }

    /// Create a new thread
    pub fn new(
        name: alloc::string::String,
        entry_point: fn(),
        stack_top: VirtAddr,
        stack_bottom: VirtAddr,
        tls_block: VirtAddr,
        privilege: ThreadPrivilege,
    ) -> Self {
        let id = NEXT_THREAD_ID.fetch_add(1, Ordering::SeqCst);

        // Set up initial context
        // Stack grows down, so initial RSP should be at top
        let context = CpuContext::new(
            VirtAddr::new(thread_entry_trampoline as u64),
            stack_top,
            privilege,
        );

        Self {
            id,
            name,
            state: ThreadState::Ready,
            context,
            stack_top,
            stack_bottom,
            kernel_stack_top: None, // Will be set separately for userspace threads
            kernel_stack_allocation: None, // No kernel stack allocation for regular threads
            tls_block,
            priority: 128,  // Default medium priority
            time_slice: 10, // Default time slice
            entry_point: Some(entry_point),
            privilege,
            has_started: false, // New thread hasn't run yet
        }
    }

    /// Create a new userspace thread
    #[allow(dead_code)]
    pub fn new_userspace(
        name: alloc::string::String,
        entry_point: VirtAddr,
        stack_top: VirtAddr,
        tls_block: VirtAddr,
    ) -> Self {
        let id = NEXT_THREAD_ID.fetch_add(1, Ordering::SeqCst);

        // For userspace threads, we'll use a simple TLS setup for now
        // TODO: Properly integrate with the TLS allocation system
        let actual_tls_block = if tls_block.is_null() {
            // Allocate a simple TLS block address for this thread
            VirtAddr::new(0x10000 + id * 0x1000)
        } else {
            tls_block
        };

        // Register this thread with the TLS system
        if let Err(e) = crate::tls::register_thread_tls(id, actual_tls_block) {
            log::warn!("Failed to register thread {} with TLS system: {}", id, e);
        }

        // Calculate stack bottom (stack grows down)
        const USER_STACK_SIZE: usize = 128 * 1024;
        let stack_bottom = stack_top - USER_STACK_SIZE as u64;

        // Set up initial context for userspace
        let context = CpuContext::new(entry_point, stack_top, ThreadPrivilege::User);

        Self {
            id,
            name,
            state: ThreadState::Ready,
            context,
            stack_top,
            stack_bottom,
            kernel_stack_top: None, // Will be set separately
            kernel_stack_allocation: None, // Will be set separately for userspace threads
            tls_block: actual_tls_block,
            priority: 128,     // Default medium priority
            time_slice: 10,    // Default time slice
            entry_point: None, // Userspace threads don't have kernel entry points
            privilege: ThreadPrivilege::User,
            has_started: false, // New thread hasn't run yet
        }
    }

    /// Get the thread ID
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Check if thread can be scheduled
    pub fn is_runnable(&self) -> bool {
        self.state == ThreadState::Ready
    }

    /// Mark thread as running
    pub fn set_running(&mut self) {
        self.state = ThreadState::Running;
    }

    /// Mark thread as ready
    pub fn set_ready(&mut self) {
        if self.state != ThreadState::Terminated {
            self.state = ThreadState::Ready;
        }
    }

    /// Mark thread as blocked
    #[allow(dead_code)]
    pub fn set_blocked(&mut self) {
        self.state = ThreadState::Blocked;
    }

    /// Mark thread as terminated
    pub fn set_terminated(&mut self) {
        self.state = ThreadState::Terminated;
    }

    /// Create a new thread with a specific ID (used for fork)
    #[allow(dead_code)]
    pub fn new_with_id(
        id: u64,
        name: alloc::string::String,
        entry_point: fn(),
        stack_top: VirtAddr,
        stack_bottom: VirtAddr,
        tls_block: VirtAddr,
        privilege: ThreadPrivilege,
    ) -> Self {
        // Set up initial context
        let context = CpuContext::new(
            VirtAddr::new(thread_entry_trampoline as u64),
            stack_top,
            privilege,
        );

        Self {
            id,
            name,
            state: ThreadState::Ready,
            context,
            stack_top,
            stack_bottom,
            kernel_stack_top: None, // Will be set separately for userspace threads
            kernel_stack_allocation: None, // No kernel stack allocation for regular threads
            tls_block,
            priority: 128,  // Default medium priority
            time_slice: 10, // Default time slice
            entry_point: Some(entry_point),
            privilege,
            has_started: false, // New thread hasn't run yet
        }
    }
}

/// Thread entry point trampoline
/// This function is called when a thread starts for the first time
extern "C" fn thread_entry_trampoline() -> ! {
    // Get current thread from TLS
    let thread_id = crate::tls::current_thread_id();

    log::debug!("Thread {} starting execution", thread_id);

    // TODO: Get thread entry point from thread structure
    // For now, we'll need to store it somewhere accessible

    // Call the actual entry point
    // thread.entry_point();

    // Thread finished, call exit syscall
    let _ = crate::syscall::handlers::sys_exit(0);

    // Should never reach here
    unreachable!("Thread exit failed");
}
