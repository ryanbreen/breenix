//! Thread management for preemptive multitasking
//!
//! This module implements real threads with preemptive scheduling,
//! building on top of the existing async executor infrastructure.
//!
//! Architecture-specific details:
//! - x86_64: Uses RIP, RSP, RFLAGS, and general purpose registers (RAX-R15)
//! - AArch64: Uses PC (ELR_EL1), SP, SPSR, and general purpose registers (X0-X30)

use core::sync::atomic::{AtomicU64, Ordering};

#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;

// Use the shared arch_stub VirtAddr for non-x86_64 architectures
#[cfg(not(target_arch = "x86_64"))]
pub use crate::memory::arch_stub::VirtAddr;

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
    /// Thread is blocked waiting for a signal (pause syscall)
    BlockedOnSignal,
    /// Thread is blocked waiting for a child to exit (waitpid syscall)
    BlockedOnChildExit,
    /// Thread is blocked waiting for a timer to expire (nanosleep syscall)
    BlockedOnTimer,
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

// =============================================================================
// x86_64 CPU Context
// =============================================================================

/// CPU context saved during context switch (x86_64)
#[cfg(target_arch = "x86_64")]
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

    /// Segment registers (for userspace support)
    pub cs: u64,
    pub ss: u64,
}

#[cfg(target_arch = "x86_64")]
impl CpuContext {
    /// Create a CpuContext from a syscall frame (captures actual register values at syscall time)
    pub fn from_syscall_frame(frame: &crate::syscall::handler::SyscallFrame) -> Self {
        Self {
            rax: frame.rax,
            rbx: frame.rbx,
            rcx: frame.rcx,
            rdx: frame.rdx,
            rsi: frame.rsi,
            rdi: frame.rdi,
            rbp: frame.rbp,
            rsp: frame.rsp,
            r8: frame.r8,
            r9: frame.r9,
            r10: frame.r10,
            r11: frame.r11,
            r12: frame.r12,
            r13: frame.r13,
            r14: frame.r14,
            r15: frame.r15,
            rip: frame.rip,
            rflags: frame.rflags,
            cs: frame.cs,
            ss: frame.ss,
        }
    }

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

// =============================================================================
// AArch64 CPU Context
// =============================================================================

/// CPU context saved during context switch (AArch64)
///
/// ARM64 calling convention (AAPCS64):
/// - X0-X7: Arguments/results (caller-saved)
/// - X8: Indirect result (caller-saved)
/// - X9-X15: Temporaries (caller-saved)
/// - X16-X17: Intra-procedure call (caller-saved)
/// - X18: Platform register (reserved)
/// - X19-X28: Callee-saved registers
/// - X29: Frame pointer (FP)
/// - X30: Link register (LR) - return address
/// - SP: Stack pointer
/// - PC: Program counter (stored in ELR_EL1 for exceptions)
#[cfg(target_arch = "aarch64")]
#[derive(Debug, Clone)]
#[repr(C)]
pub struct CpuContext {
    // All general-purpose registers x0-x30
    // We save ALL registers (not just callee-saved) because:
    // 1. Fork needs x0 for return value (child gets 0, parent gets child PID)
    // 2. Kernel thread preemption can happen mid-loop, caller-saved registers
    //    (x0-x18) may contain loop variables, pointers, etc. that must be preserved.
    pub x0: u64,
    pub x1: u64,
    pub x2: u64,
    pub x3: u64,
    pub x4: u64,
    pub x5: u64,
    pub x6: u64,
    pub x7: u64,
    pub x8: u64,
    pub x9: u64,
    pub x10: u64,
    pub x11: u64,
    pub x12: u64,
    pub x13: u64,
    pub x14: u64,
    pub x15: u64,
    pub x16: u64,
    pub x17: u64,
    pub x18: u64,
    // Callee-saved registers
    pub x19: u64,
    pub x20: u64,
    pub x21: u64,
    pub x22: u64,
    pub x23: u64,
    pub x24: u64,
    pub x25: u64,
    pub x26: u64,
    pub x27: u64,
    pub x28: u64,
    pub x29: u64,  // Frame pointer (FP)
    pub x30: u64,  // Link register (LR) - return address for context switch

    /// Stack pointer
    pub sp: u64,

    // For userspace threads:
    /// User stack pointer (SP_EL0)
    pub sp_el0: u64,
    /// Exception return address (user PC)
    pub elr_el1: u64,
    /// Saved program status (includes EL0 mode bits)
    pub spsr_el1: u64,
}

#[cfg(target_arch = "aarch64")]
impl CpuContext {
    /// Create a new CPU context for a thread entry point
    pub fn new(entry_point: VirtAddr, stack_pointer: VirtAddr, privilege: ThreadPrivilege) -> Self {
        match privilege {
            ThreadPrivilege::Kernel => Self::new_kernel_thread(entry_point.as_u64(), stack_pointer.as_u64()),
            ThreadPrivilege::User => Self::new_user_thread(entry_point.as_u64(), stack_pointer.as_u64(), 0),
        }
    }

    /// Create a context for a new kernel thread.
    ///
    /// The thread will start executing at `entry_point` with the given stack.
    pub fn new_kernel_thread(entry_point: u64, stack_top: u64) -> Self {
        Self {
            x0: 0,  // Result register (not used for initial context)
            x1: 0, x2: 0, x3: 0, x4: 0,
            x5: 0, x6: 0, x7: 0, x8: 0,
            x9: 0, x10: 0, x11: 0, x12: 0,
            x13: 0, x14: 0, x15: 0, x16: 0,
            x17: 0, x18: 0,
            x19: 0, x20: 0, x21: 0, x22: 0,
            x23: 0, x24: 0, x25: 0, x26: 0,
            x27: 0, x28: 0, x29: 0,
            x30: entry_point,  // LR = entry point (ret will jump here)
            sp: stack_top,
            sp_el0: 0,
            elr_el1: 0,
            // SPSR with EL1h mode, interrupts masked initially
            spsr_el1: 0x3c5, // EL1h, DAIF masked
        }
    }

    /// Create a context for a new userspace thread.
    ///
    /// The thread will start executing at `entry_point` in EL0 with the given
    /// user stack. Kernel stack is used for exception handling.
    pub fn new_user_thread(
        entry_point: u64,
        user_stack_top: u64,
        kernel_stack_top: u64,
    ) -> Self {
        Self {
            x0: 0,  // Result register (starts at 0 for new threads)
            x1: 0, x2: 0, x3: 0, x4: 0,
            x5: 0, x6: 0, x7: 0, x8: 0,
            x9: 0, x10: 0, x11: 0, x12: 0,
            x13: 0, x14: 0, x15: 0, x16: 0,
            x17: 0, x18: 0,
            x19: 0, x20: 0, x21: 0, x22: 0,
            x23: 0, x24: 0, x25: 0, x26: 0,
            x27: 0, x28: 0, x29: 0, x30: 0,
            sp: kernel_stack_top,      // Kernel SP for exceptions
            sp_el0: user_stack_top,    // User stack pointer
            elr_el1: entry_point,      // Where to jump in userspace
            // SPSR for EL0: mode=0 (EL0t), DAIF clear (interrupts enabled)
            spsr_el1: 0x0,             // EL0t with interrupts enabled
        }
    }

    /// Create a CpuContext from an ARM64 exception frame (captures actual register values at syscall time)
    ///
    /// This captures the userspace context from the exception frame saved by the syscall entry.
    /// The exception frame contains all registers as they were at the time of the SVC instruction.
    pub fn from_aarch64_frame(frame: &crate::arch_impl::aarch64::exception_frame::Aarch64ExceptionFrame, user_sp: u64) -> Self {
        Self {
            // All general-purpose registers from the exception frame
            x0: frame.x0,
            x1: frame.x1,
            x2: frame.x2,
            x3: frame.x3,
            x4: frame.x4,
            x5: frame.x5,
            x6: frame.x6,
            x7: frame.x7,
            x8: frame.x8,
            x9: frame.x9,
            x10: frame.x10,
            x11: frame.x11,
            x12: frame.x12,
            x13: frame.x13,
            x14: frame.x14,
            x15: frame.x15,
            x16: frame.x16,
            x17: frame.x17,
            x18: frame.x18,
            x19: frame.x19,
            x20: frame.x20,
            x21: frame.x21,
            x22: frame.x22,
            x23: frame.x23,
            x24: frame.x24,
            x25: frame.x25,
            x26: frame.x26,
            x27: frame.x27,
            x28: frame.x28,
            x29: frame.x29,  // Frame pointer
            x30: frame.x30,  // Link register
            sp: 0,           // Kernel SP will be set when scheduling
            sp_el0: user_sp, // User stack pointer (passed separately since it's in SP_EL0)
            elr_el1: frame.elr, // Return address (where to resume after syscall)
            spsr_el1: frame.spsr, // Saved program status
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

    /// Is the thread blocked inside a syscall? (for pause/waitpid)
    /// When true, the thread should resume in kernel mode, not userspace.
    /// This prevents the scheduler from restoring stale userspace context.
    pub blocked_in_syscall: bool,

    /// Saved userspace context when blocked in syscall (for signal delivery)
    /// When a thread blocks in a syscall (pause/waitpid), we save the pre-syscall
    /// userspace context here. If a signal arrives while blocked, we use this
    /// context to deliver the signal handler (with RAX = -EINTR).
    pub saved_userspace_context: Option<CpuContext>,

    /// Absolute monotonic wake time in nanoseconds (for nanosleep)
    /// When set, the scheduler will unblock this thread when the monotonic
    /// clock reaches this value.
    pub wake_time_ns: Option<u64>,
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
            blocked_in_syscall: self.blocked_in_syscall,
            saved_userspace_context: self.saved_userspace_context.clone(),
            wake_time_ns: self.wake_time_ns,
        }
    }
}

impl Thread {
    // =========================================================================
    // x86_64-specific constructors
    // =========================================================================

    /// Create a new kernel thread with an argument (x86_64)
    ///
    /// On x86_64, the argument is passed in RDI per System V ABI.
    #[cfg(target_arch = "x86_64")]
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
            blocked_in_syscall: false, // New thread is not blocked in syscall
            saved_userspace_context: None,
            wake_time_ns: None,
        })
    }

    /// Create a new kernel thread with an argument (AArch64)
    ///
    /// On AArch64, the argument is passed in X0 per AAPCS64.
    #[cfg(target_arch = "aarch64")]
    pub fn new_kernel(
        name: alloc::string::String,
        entry_point: extern "C" fn(u64) -> !,
        arg: u64,
    ) -> Result<Self, &'static str> {
        let id = NEXT_THREAD_ID.fetch_add(1, Ordering::SeqCst);

        // Allocate a kernel stack
        const KERNEL_STACK_SIZE: usize = 16 * 1024; // 16 KiB
        let stack = crate::memory::alloc_kernel_stack(KERNEL_STACK_SIZE)
            .ok_or("Failed to allocate kernel stack")?;

        let stack_top = stack.top();
        let stack_bottom = stack.bottom();

        // Set up initial context for kernel thread
        // For AArch64, we create a context where the entry point is in X30 (LR)
        // and the argument will be passed in X0 when we set up a proper trampoline
        let mut context = CpuContext::new_kernel_thread(entry_point as u64, stack_top.as_u64());

        // ARM64: We can't directly set X0 in callee-saved context.
        // For kernel threads with arguments, we need the assembly trampoline
        // to load the argument from somewhere. For now, store it in x19 (callee-saved)
        // and have the entry point read it from there.
        context.x19 = arg;

        // Kernel threads don't need TLS
        let tls_block = VirtAddr::new(0);

        Ok(Self {
            id,
            name,
            state: ThreadState::Ready,
            context,
            stack_top,
            stack_bottom,
            kernel_stack_top: Some(stack_top),
            kernel_stack_allocation: Some(stack),
            tls_block,
            priority: 64,
            time_slice: 20,
            entry_point: None,
            privilege: ThreadPrivilege::Kernel,
            has_started: false,
            blocked_in_syscall: false,
            saved_userspace_context: None,
            wake_time_ns: None,
        })
    }

    /// Create a new thread (x86_64)
    #[cfg(target_arch = "x86_64")]
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
            blocked_in_syscall: false, // New thread is not blocked in syscall
            saved_userspace_context: None,
            wake_time_ns: None,
        }
    }

    /// Create a new thread (AArch64)
    ///
    /// Note: Thread entry trampolines are not yet implemented for AArch64.
    /// This is a stub for future implementation.
    #[cfg(target_arch = "aarch64")]
    #[allow(dead_code)]
    pub fn new(
        name: alloc::string::String,
        entry_point: fn(),
        stack_top: VirtAddr,
        stack_bottom: VirtAddr,
        tls_block: VirtAddr,
        privilege: ThreadPrivilege,
    ) -> Self {
        let id = NEXT_THREAD_ID.fetch_add(1, Ordering::SeqCst);

        // Set up initial context - entry point goes directly in X30 (LR)
        let context = CpuContext::new(
            VirtAddr::new(entry_point as u64),
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
            kernel_stack_top: None,
            kernel_stack_allocation: None,
            tls_block,
            priority: 128,
            time_slice: 10,
            entry_point: Some(entry_point),
            privilege,
            has_started: false,
            blocked_in_syscall: false,
            saved_userspace_context: None,
            wake_time_ns: None,
        }
    }

    /// Create a new userspace thread (x86_64)
    #[cfg(target_arch = "x86_64")]
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
            blocked_in_syscall: false, // New thread is not blocked in syscall
            saved_userspace_context: None,
            wake_time_ns: None,
        }
    }

    /// Create a new userspace thread (AArch64)
    ///
    /// Note: TLS support is not yet implemented for AArch64.
    #[cfg(target_arch = "aarch64")]
    #[allow(dead_code)]
    pub fn new_userspace(
        name: alloc::string::String,
        entry_point: VirtAddr,
        stack_top: VirtAddr,
        tls_block: VirtAddr,
    ) -> Self {
        let id = NEXT_THREAD_ID.fetch_add(1, Ordering::SeqCst);

        // For AArch64, use a simple TLS placeholder
        let actual_tls_block = if tls_block.is_null() {
            VirtAddr::new(0x10000 + id * 0x1000)
        } else {
            tls_block
        };

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
            kernel_stack_top: None,
            kernel_stack_allocation: None,
            tls_block: actual_tls_block,
            priority: 128,
            time_slice: 10,
            entry_point: None,
            privilege: ThreadPrivilege::User,
            has_started: false,
            blocked_in_syscall: false,
            saved_userspace_context: None,
            wake_time_ns: None,
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

    /// Create a new thread with a specific ID (used for fork) - x86_64 only
    #[cfg(target_arch = "x86_64")]
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
            blocked_in_syscall: false, // New thread is not blocked in syscall
            saved_userspace_context: None,
            wake_time_ns: None,
        }
    }

    /// Create a new thread with a specific ID (used for fork) - AArch64
    #[cfg(target_arch = "aarch64")]
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
        // Set up initial context - entry point goes directly in X30 (LR)
        let context = CpuContext::new(
            VirtAddr::new(entry_point as u64),
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
            kernel_stack_top: None,
            kernel_stack_allocation: None,
            tls_block,
            priority: 128,
            time_slice: 10,
            entry_point: Some(entry_point),
            privilege,
            has_started: false,
            blocked_in_syscall: false,
            saved_userspace_context: None,
            wake_time_ns: None,
        }
    }
}

/// Thread entry point trampoline (x86_64)
///
/// This function is called when a thread starts for the first time.
/// It retrieves the actual entry point from per-CPU data and calls it.
#[cfg(target_arch = "x86_64")]
extern "C" fn thread_entry_trampoline() -> ! {
    // Get current thread from per-CPU data
    let entry_point = crate::per_cpu::current_thread()
        .and_then(|t| t.entry_point.take());

    if let Some(entry_fn) = entry_point {
        log::debug!("Thread starting execution via trampoline");

        // Call the actual entry point
        entry_fn();

        // If the entry point returns, the thread is done
        log::debug!("Thread entry point returned");
    } else {
        log::error!("Thread has no entry point!");
    }

    // Thread finished (or had no entry point), call exit
    let _ = crate::syscall::handlers::sys_exit(0);

    // Should never reach here
    unreachable!("Thread exit failed");
}
