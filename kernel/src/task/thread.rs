//! Thread management for preemptive multitasking
//!
//! This module implements real threads with preemptive scheduling,
//! building on top of the existing async executor infrastructure.

use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::VirtAddr;

/// Global thread ID counter
static NEXT_THREAD_ID: AtomicU64 = AtomicU64::new(1); // 0 is reserved for kernel thread

/// Thread states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    /// Thread is currently running on CPU
    Running,
    /// Thread is ready to run and in scheduler queue
    Ready,
    /// Thread has terminated
    Terminated,
    /// Thread is blocked waiting for something
    Blocked(BlockedReason),
}

/// Reasons why a thread might be blocked
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockedReason {
    /// Waiting for a child process to exit
    Wait,
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
                ThreadPrivilege::User => 0x202,   // IF flag set + mandatory bit 1 - interrupts enabled
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
    
    /// First run flag - false until thread executes at least one userspace instruction
    /// NOTE: Idle thread starts with first_run = true so timer ISR can trigger scheduling
    pub first_run: bool,
    
    /// Number of timer ticks this thread has been scheduled for
    pub ticks_run: u32,
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
            tls_block: self.tls_block,
            priority: self.priority,
            time_slice: self.time_slice,
            entry_point: self.entry_point, // fn pointers can be copied
            privilege: self.privilege,
            first_run: self.first_run,
            ticks_run: self.ticks_run,
        }
    }
}

impl Thread {
    
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
            tls_block,
            priority: 128, // Default medium priority
            time_slice: 10, // Default time slice
            entry_point: Some(entry_point),
            privilege,
            first_run: false, // Initialize to false for all threads
            ticks_run: 0,     // No ticks run yet
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
    
    /// Mark thread as blocked
    pub fn set_blocked(&mut self, reason: BlockedReason) {
        self.state = ThreadState::Blocked(reason);
    }
    
    /// Mark thread as running
    pub fn set_running(&mut self) {
        self.state = ThreadState::Running;
    }
    
    /// Mark thread as ready
    pub fn set_ready(&mut self) {
        match self.state {
            ThreadState::Terminated => {}, // Don't revive terminated threads
            _ => self.state = ThreadState::Ready,
        }
    }
    
    /// Mark thread as terminated
    pub fn set_terminated(&mut self) {
        // 3-D: Thread finalizer trace - thread marked terminated
        #[cfg(feature = "testing")]
        crate::serial_println!("SCHED_TERMINATE tid={}", self.id);
        
        self.state = ThreadState::Terminated;
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