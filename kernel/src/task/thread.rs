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
pub use x86_64::VirtAddr;

// Use the shared arch_stub VirtAddr for non-x86_64 architectures
#[cfg(not(target_arch = "x86_64"))]
pub use crate::memory::arch_stub::VirtAddr;

/// Global thread ID counter
static NEXT_THREAD_ID: AtomicU64 = AtomicU64::new(1); // 0 is the no-thread sentinel and is never allocated

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
    /// Thread is blocked waiting for device I/O completion (AHCI, etc.)
    BlockedOnIO,
    /// Thread has terminated
    Terminated,
}

impl ThreadState {
    /// True for every variant the scheduler treats as parked waiting on
    /// some external event, as opposed to `Running`, `Ready`, or
    /// `Terminated`. `schedule()` and `unblock()` (task/scheduler.rs) both
    /// switch on this exact five-variant set in several places; this
    /// predicate exists so the next missed site is impossible instead of a
    /// sixth hand-copied match arm (#673 review, m4). Some call sites
    /// (per_cpu::can_schedule(), unblock()) recognize a deliberately
    /// DIFFERENT subset for their own documented reasons and do not use
    /// this predicate -- see their own comments.
    #[inline(always)]
    pub fn is_blocked(self) -> bool {
        matches!(
            self,
            ThreadState::Blocked
                | ThreadState::BlockedOnSignal
                | ThreadState::BlockedOnChildExit
                | ThreadState::BlockedOnTimer
                | ThreadState::BlockedOnIO
        )
    }
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
    /// Explicit ARCH_SET_FS value, including a valid zero base.
    pub user_fs_base: u64,
    pub user_fs_base_set: bool,
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
            user_fs_base: crate::per_cpu::current_thread()
                .map_or(0, |thread| thread.context.user_fs_base),
            user_fs_base_set: crate::per_cpu::current_thread()
                .is_some_and(|thread| thread.context.user_fs_base_set),
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
            user_fs_base: 0,
            user_fs_base_set: false,
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
    pub x29: u64, // Frame pointer (FP)
    pub x30: u64, // Link register (LR) - return address for context switch

    /// Stack pointer
    pub sp: u64,

    // For userspace threads:
    /// User stack pointer (SP_EL0)
    pub sp_el0: u64,
    /// Exception return address (user PC)
    pub elr_el1: u64,
    /// Saved program status (includes EL0 mode bits)
    pub spsr_el1: u64,
    /// Thread pointer (TPIDR_EL0) - used by musl/libc for Thread Local Storage
    pub tpidr_el0: u64,

    /// Identity word. Stamped by every constructor, never written again, and
    /// checked by the ret-based dispatch admission before a raw pointer into
    /// this row is used to restore callee-saved registers, SP and a link
    /// register. A pointer that no longer names a live `CpuContext` — a row
    /// whose `Vec` buffer moved or was freed and reused — almost never carries
    /// the word, so the admission refuses instead of restoring whatever the
    /// memory now holds. Placed last so every offset the assembly knows
    /// (x19@152 .. elr_el1@264) is unchanged; the const-asserts below pin that.
    pub magic: u64,
}

/// The value `CpuContext::magic` carries for the whole life of a context.
#[cfg(target_arch = "aarch64")]
pub const CPU_CONTEXT_MAGIC: u64 = 0x4252_5843_5458_3031; // "BRXCTX01"

#[cfg(target_arch = "aarch64")]
impl CpuContext {
    /// True when this row still looks like a live, kernel-constructed context.
    #[inline(always)]
    pub fn identity_is_intact(&self) -> bool {
        self.magic == CPU_CONTEXT_MAGIC
    }
}

#[cfg(target_arch = "aarch64")]
impl CpuContext {
    /// Create a new CPU context for a thread entry point
    pub fn new(entry_point: VirtAddr, stack_pointer: VirtAddr, privilege: ThreadPrivilege) -> Self {
        match privilege {
            ThreadPrivilege::Kernel => {
                Self::new_kernel_thread(entry_point.as_u64(), stack_pointer.as_u64())
            }
            ThreadPrivilege::User => {
                Self::new_user_thread(entry_point.as_u64(), stack_pointer.as_u64(), 0)
            }
        }
    }

    /// Create a context for a new kernel thread.
    ///
    /// The thread will start executing at `entry_point` with the given stack.
    pub fn new_kernel_thread(entry_point: u64, stack_top: u64) -> Self {
        Self {
            x0: 0, // Result register (not used for initial context)
            x1: 0,
            x2: 0,
            x3: 0,
            x4: 0,
            x5: 0,
            x6: 0,
            x7: 0,
            x8: 0,
            x9: 0,
            x10: 0,
            x11: 0,
            x12: 0,
            x13: 0,
            x14: 0,
            x15: 0,
            x16: 0,
            x17: 0,
            x18: 0,
            x19: 0,
            x20: 0,
            x21: 0,
            x22: 0,
            x23: 0,
            x24: 0,
            x25: 0,
            x26: 0,
            x27: 0,
            x28: 0,
            x29: 0,
            x30: entry_point, // LR = entry point (ret will jump here)
            sp: stack_top,
            sp_el0: 0,
            elr_el1: 0,
            // SPSR with EL1h mode and IRQs enabled
            spsr_el1: 0x5, // EL1h, DAIF clear
            tpidr_el0: 0,
            magic: CPU_CONTEXT_MAGIC,
        }
    }

    /// Create a context for a new userspace thread.
    ///
    /// The thread will start executing at `entry_point` in EL0 with the given
    /// user stack. Kernel stack is used for exception handling.
    pub fn new_user_thread(entry_point: u64, user_stack_top: u64, kernel_stack_top: u64) -> Self {
        Self {
            x0: 0, // Result register (starts at 0 for new threads)
            x1: 0,
            x2: 0,
            x3: 0,
            x4: 0,
            x5: 0,
            x6: 0,
            x7: 0,
            x8: 0,
            x9: 0,
            x10: 0,
            x11: 0,
            x12: 0,
            x13: 0,
            x14: 0,
            x15: 0,
            x16: 0,
            x17: 0,
            x18: 0,
            x19: 0,
            x20: 0,
            x21: 0,
            x22: 0,
            x23: 0,
            x24: 0,
            x25: 0,
            x26: 0,
            x27: 0,
            x28: 0,
            x29: 0,
            x30: 0,
            sp: kernel_stack_top,   // Kernel SP for exceptions
            sp_el0: user_stack_top, // User stack pointer
            elr_el1: entry_point,   // Where to jump in userspace
            // SPSR for EL0: mode=0 (EL0t), DAIF clear (interrupts enabled)
            spsr_el1: 0x0, // EL0t with interrupts enabled
            tpidr_el0: 0,  // TLS pointer, set by musl during __init_tls
            magic: CPU_CONTEXT_MAGIC,
        }
    }

    /// Create a CpuContext from an ARM64 exception frame (captures actual register values at syscall time)
    ///
    /// This captures the userspace context from the exception frame saved by the syscall entry.
    /// The exception frame contains all registers as they were at the time of the SVC instruction.
    pub fn from_aarch64_frame(
        frame: &crate::arch_impl::aarch64::exception_frame::Aarch64ExceptionFrame,
        user_sp: u64,
    ) -> Self {
        // Read TPIDR_EL0 (user TLS pointer) so forked children inherit it
        let tpidr: u64;
        unsafe {
            core::arch::asm!("mrs {}, tpidr_el0", out(reg) tpidr, options(nomem, nostack));
        }
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
            x29: frame.x29,       // Frame pointer
            x30: frame.x30,       // Link register
            sp: 0,                // Kernel SP will be set when scheduling
            sp_el0: user_sp,      // User stack pointer (passed separately since it's in SP_EL0)
            elr_el1: frame.elr,   // Return address (where to resume after syscall)
            spsr_el1: frame.spsr, // Saved program status
            tpidr_el0: tpidr,     // User TLS pointer (inherited by forked child)
            magic: CPU_CONTEXT_MAGIC,
        }
    }
}

/// The timer-heap pops seen during one timed wait. Armed with the wait's
/// deadline when the wait is published; only a heap entry carrying that
/// deadline belongs to the wait. An entry left by an earlier wait on the same
/// thread carries a different deadline and is counted, not attributed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimerPopRecord {
    /// The deadline the current wait armed, in monotonic nanoseconds.
    pub deadline_ns: u64,
    /// What the pop of this wait's own entry saw.
    pub own_entry: TimerPop,
    /// Expired entries from earlier waits that were discarded during this one.
    pub stale_entries: u32,
}

impl TimerPopRecord {
    pub fn armed(deadline_ns: u64) -> Self {
        Self {
            deadline_ns,
            own_entry: TimerPop::NotPopped,
            stale_entries: 0,
        }
    }
}

/// What `Scheduler::wake_expired_timers` saw when it popped a wait's own entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerPop {
    /// The entry has not expired yet.
    NotPopped,
    /// `wake_time_ns` still held the deadline, so the pop ended the wait.
    WakeTimeSet,
    /// `wake_time_ns` had been cleared, so the pop was a no-op. A wait still
    /// blocked at that point has lost its deadline.
    WakeTimeCleared,
}

/// CPU time of one process: what every copy of every thread it has had was
/// charged, and what the children it has waited for used. Shared by the rows
/// of a thread group, so each thread reads the same totals. Atomic so the
/// scheduler charges it without the process manager lock.
///
/// Two clocks are kept. Timer ticks (`own`, `children`) feed /proc and the CPU
/// resource limit. Nanoseconds split between user and system mode, stamped at
/// kernel entry and exit (`Thread::switch_timer_mode`), feed getrusage, times,
/// the CPU-time clocks and timers, and ITIMER_VIRTUAL and ITIMER_PROF.
#[derive(Default)]
pub struct CpuAccount {
    own: AtomicU64,
    children: AtomicU64,
    pub user_ns: AtomicU64,
    pub system_ns: AtomicU64,
    children_user_ns: AtomicU64,
    children_system_ns: AtomicU64,
}

impl CpuAccount {
    /// User and system nanoseconds charged so far.
    pub fn split_ns(&self) -> (u64, u64) {
        (self.user_ns.load(Ordering::Relaxed), self.system_ns.load(Ordering::Relaxed))
    }

    /// User and system nanoseconds of the children waited for.
    pub fn children_split_ns(&self) -> (u64, u64) {
        (
            self.children_user_ns.load(Ordering::Relaxed),
            self.children_system_ns.load(Ordering::Relaxed),
        )
    }

    /// Add a waited-for child's user and system time, its own and its
    /// children's, in nanoseconds.
    pub fn add_children_ns(&self, user_ns: u64, system_ns: u64) {
        self.children_user_ns.fetch_add(user_ns, Ordering::Relaxed);
        self.children_system_ns.fetch_add(system_ns, Ordering::Relaxed);
    }

    pub fn charge(&self, ticks: u64) {
        self.own.fetch_add(ticks, Ordering::Relaxed);
    }

    pub fn ticks(&self) -> u64 {
        self.own.load(Ordering::Relaxed)
    }

    /// Add a waited-for child's time, its own and its children's.
    pub fn add_children(&self, ticks: u64) {
        self.children.fetch_add(ticks, Ordering::Relaxed);
    }

    pub fn children_ticks(&self) -> u64 {
        self.children.load(Ordering::Relaxed)
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

    /// x87/SSE registers while the thread is not current on a CPU
    #[cfg(target_arch = "x86_64")]
    pub fpu: crate::arch_impl::x86_64::fpu::FpuState,

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

    /// Was this thread's context saved by schedule_from_kernel() (inline schedule)?
    /// When true, the thread should be resumed via ret-based dispatch (restore
    /// callee-saved regs + SP, then ret to x30) instead of ERET. This avoids
    /// the CPU 0 IRQ death bug where ERET dispatches a thread into code that
    /// re-masks DAIF.I (e.g., inside a without_interrupts block).
    /// Matches Linux's cpu_switch_to approach: kernel-to-kernel switches use ret.
    pub saved_by_inline_schedule: bool,

    /// Kernel PSTATE captured by the ret-based inline schedule path.
    /// `context.spsr_el1` remains paired with `context.elr_el1`; inline resume
    /// metadata must not turn a saved user PC into an apparent EL1 return.
    pub inline_schedule_spsr: u64,

    /// Diagnostic: pre-save ELR observed immediately before the inline asm save.
    /// This preserves evidence when the inline resume PC differs from stale ELR.
    pub inline_schedule_prev_elr: u64,

    /// Diagnostic: caller LR saved in the suspended schedule_from_kernel() frame.
    /// Used to detect whether the inline-saved kernel frame is already corrupt
    /// by the time a later exception save overwrites this thread's context.
    pub inline_schedule_caller_lr: u64,

    /// Diagnostic: original SP of the suspended schedule_from_kernel() frame.
    /// Used to distinguish dormant-frame overwrite from later publication of an
    /// incorrect thread.context.sp value.
    pub inline_schedule_saved_sp: u64,

    /// Saved userspace context when blocked in syscall (for signal delivery)
    /// When a thread blocks in a syscall (pause/waitpid), we save the pre-syscall
    /// userspace context here. If a signal arrives while blocked, we use this
    /// context to deliver the signal handler (with RAX = -EINTR).
    pub saved_userspace_context: Option<CpuContext>,

    /// Absolute monotonic wake time in nanoseconds (for nanosleep)
    /// When set, the scheduler will unblock this thread when the monotonic
    /// clock reaches this value.
    pub wake_time_ns: Option<u64>,
    /// Re-evaluate this wait when CLOCK_REALTIME changes.
    pub realtime_sleep: bool,

    /// What `Scheduler::wake_expired_timers` has done with the timer-heap
    /// entries of this thread's current timed wait, keyed by the deadline the
    /// wait armed. `None` while the current wait has no deadline. Read by the
    /// futex timed-wait record (#608 F4).
    pub timer_pop: Option<TimerPopRecord>,

    /// Counter-backed tick timestamp when this thread started its current run.
    pub run_start_ticks: u64,

    /// Accumulated CPU ticks consumed by this thread across all scheduling quanta.
    /// Updated in schedule() when the thread is switched out.
    pub cpu_ticks_total: u64,

    pub resource_limits: Option<alloc::sync::Arc<crate::process::limits::Limits>>,
    /// The process account every tick in `cpu_ticks_total` is also charged
    /// to, so a process's CPU time outlives its threads. Attached when the
    /// thread becomes a process's main thread; None for kernel threads.
    pub cpu_account: Option<alloc::sync::Arc<CpuAccount>>,
    pub signals: alloc::sync::Arc<crate::signal::ThreadSignals>,
    pub signal_timers: Option<alloc::sync::Arc<crate::signal::IntervalTimers>>,

    /// Owner process PID (for mapping thread CPU time to process in btop).
    /// None for idle threads and kernel-internal threads not associated with a process.
    pub owner_pid: Option<u64>,

    /// Last known good TTBR0/CR3 value for this thread's userspace address space.
    /// On ARM64 this lets the scheduler resume blocked-in-syscall threads without
    /// taking PROCESS_MANAGER in the hot dispatch path when the lock is contended.
    pub cached_ttbr0: u64,

    /// How many times this thread has parked on one of the kernel's halt
    /// primitives -- `crate::arch_halt_with_interrupts`, `crate::arch_halt`,
    /// the private one in `graphics/render_task.rs` -- or on one of the two
    /// loops that park on a raw `enable_and_hlt`/`wfi` and bump this by hand
    /// at 3 call sites (`task/executor.rs`'s `sleep_if_idle`, once per arch
    /// arm, and `task/spawn.rs`'s `idle_thread_fn`). Between them those are
    /// the park points of every blocking wait loop in the kernel (#772).
    /// Ruling R113 (2026-09-03) retired the proxy; the split this count feeds
    /// is its replacement.
    /// `crate::arch_halt_with_interrupts` carries the full park census,
    /// including the two families of halt loop that are NOT counted -- the 6
    /// raw `enable_and_hlt` idle and terminal loops, and the bare-halt
    /// instruction sites beyond them.
    // claim-lint:ok: 25 of 25 arch_halt_with_interrupts call sites and 24 of 24
    // arch_halt call sites under kernel/src reach a bump, counted by grep in
    // this slot.
    ///
    /// The dispatch mark stamps this value at dispatch; the save site reads it
    /// again. A save whose frame is byte-identical to the mark therefore splits
    /// two ways that used to be recorded as one: the thread went round its wait
    /// loop and re-parked on the same halt (the count advanced), or it retired
    /// no instructions (the count is unchanged).
    ///
    /// Bumped in thread context, read from the interrupt-return path on the same
    /// CPU, so it is an atomic rather than a plain `u64`. Relaxed ordering is
    /// enough: the value is only ever compared against a stamp of itself.
    pub wait_loop_iters: AtomicU64,

    /// Kill custody: the low bits count the kernel sections this thread is
    /// inside that a SIGKILL must let it finish, and `KILL_CLAIMED` records
    /// that `kill_process_now` has taken the thread, and `KILL_PENDING` that
    /// it left SIGKILL pending instead. Entry and claim are both
    /// compare-and-swap on this one word, so a thread is never killed inside a
    /// section and never enters one once claimed. See `KillCustody`.
    pub kill_custody: AtomicU64,

    /// Optional CPU target: `Some(pin)` pins the thread; the empty state permits
    /// migration.
    ///
    /// The pin carries *why* it exists, because the two reasons need opposite
    /// treatment when the home CPU stops dispatching. See `CpuPin`.
    ///
    /// Nothing in this tree stamps a pin yet: `spawn_on_cpu` is the only
    /// function that writes this field and it has 0 callers, as does the
    /// `kthread_run_on_cpu` that wraps it. 14 of 15 sites that build a
    /// `Thread` set this to `None` outright -- both child-creation paths
    /// included, because a pin is not inherited -- and the 15th, the `Clone`
    /// impl, carries whatever its source held, which is the empty state today.
    ///
    /// The field lives in two copies -- this process-table row and the
    /// publication clone `publish_to_scheduler` hands the scheduler -- and the
    /// scheduler's copy is the one every placement decision reads:
    /// `find_target_cpu_for_wakeup`, the park, and `add_thread_inner`'s
    /// publication discard all reach it through `self.get_thread*`, never
    /// through the process table. Nothing clears a live pin, so the two cannot
    /// drift while a thread runs; the one write that corrects a pin
    /// (`add_thread_inner`, on a pin the online bound rejected) happens after
    /// the publication clone has been pushed, so it corrects the copy the
    /// scheduler reads.
    // claim-lint:ok: 14 of 15 `Thread` build sites in kernel/src carry
    // `cpu_affinity: None`, and `spawn_on_cpu` has 1 call site
    // (`kthread::kthread_run_on_cpu`) which itself has 0 -- all counted by grep
    // over kernel/src in this round.
    pub cpu_affinity: Option<CpuPin>,
}

/// Why a thread is pinned to one CPU, alongside which CPU that is.
///
/// The kind is not decoration: it decides what happens when the home CPU stops
/// accepting wakeups.
///
/// * `per_cpu_worker` -- the thread services state that lives in that CPU's
///   per-CPU block (`ksoftirqd/N` and its pending-softirq bitmap). Running it
///   anywhere else reads the wrong bitmap, so migrating it is not a legal
///   disposition; if its home CPU is offline or stalled it stays parked until
///   the home CPU comes back. Linux parks per-CPU kthreads on CPU-down for the
///   same reason.
/// * not a worker -- a temporary hold pen: placement only, released by whoever
///   set it. Parking one of those would strand whatever the pen is holding, so
///   a hold-pen pin is retained on its queue rather than parked.
///
/// The dispositions those two kinds imply are not implemented here. This branch
/// lands the representation and the one placement arm that reads it; the wake
/// filter, the park protocol and the steal/reclaim dispositions are separate
/// changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuPin {
    /// The CPU this thread must run on.
    pub cpu: usize,
    /// Whether the pin exists because the work itself is CPU-local.
    pub per_cpu_worker: bool,
}

/// `CpuPin` values minted since boot, by either constructor.
///
/// Monotonic: it is never decremented, so it over-counts a pin that was built
/// and then dropped or cleared, and it under-counts by 0. A reading of 0 is
/// therefore a statement that 0 `Thread::cpu_affinity` fields in the kernel
/// hold `Some`, which is what `Scheduler::retain_cpu_affine_thread` reads it
/// for: on that reading the guard answers "no constraint" from 1 relaxed load
/// and 1 compare, instead of searching `self.threads` once per migration site.
/// claim-lint:ok: 2 of 2 constructors of `CpuPin` increment this and 0
/// `CpuPin { .. }` literals exist in kernel/src outside the type's own
/// definition, both counted by
/// `tests/loopback_pump_structure.rs::every_cpu_pin_is_minted_by_a_counting_constructor`
///
/// The increment happens inside the constructor, before the value can be
/// stored into a thread row, and a row only becomes visible to another CPU
/// through the scheduler lock -- so the lock's release/acquire pair is the edge
/// that keeps a live pin from being paired with a 0 reading here, and
/// `Relaxed` is enough for the counter's own accesses.
pub static CPU_PINS_STAMPED: AtomicU64 = AtomicU64::new(0);

impl CpuPin {
    /// A pin whose work lives in `cpu`'s per-CPU state.
    pub fn per_cpu_worker(cpu: usize) -> Self {
        CPU_PINS_STAMPED.fetch_add(1, Ordering::Relaxed);
        Self {
            cpu,
            per_cpu_worker: true,
        }
    }

    /// A hold-pen pin: placement only, released by whoever set it.
    ///
    /// No caller yet -- the aarch64 testing-profile loader that stages user
    /// threads on the boot CPU is a separate change. It is defined here because
    /// the kind is what makes the two pins distinguishable at all, and a pin
    /// type with only one kind cannot carry the distinction the reclaim
    /// disposition will read.
    pub fn hold_pen(cpu: usize) -> Self {
        CPU_PINS_STAMPED.fetch_add(1, Ordering::Relaxed);
        Self {
            cpu,
            per_cpu_worker: false,
        }
    }
}

/// Set in `Thread::kill_custody` once a kill has claimed the thread.
const KILL_CLAIMED: u64 = 1 << 63;

/// Set in `Thread::kill_custody` once a kill has been left pending for the
/// thread because it was inside a section: see `mark_kill_pending`.
const KILL_PENDING: u64 = 1 << 62;

/// The bits of `Thread::kill_custody` that count open sections.
const CUSTODY_COUNT: u64 = !(KILL_CLAIMED | KILL_PENDING);

impl Thread {
    /// Charge the time since `run_start_ticks` to this thread, its process's
    /// CPU account and its resource limits, and start the next interval at
    /// `now`. Called before blocking, switching away or exiting.
    pub fn charge_cpu(&mut self, now: u64) {
        self.charge_timer_cpu();
        // A remote CPU may read a counter behind this thread's last CPU.
        let ran = now.saturating_sub(self.run_start_ticks);
        self.cpu_ticks_total = self.cpu_ticks_total.saturating_add(ran);
        self.run_start_ticks = self.run_start_ticks.max(now);
        if let Some(account) = &self.cpu_account {
            account.charge(ran);
        }
        if let Some(limits) = &self.resource_limits {
            limits.charge_cpu(ran);
        }
    }

    /// Charge the interval this thread is running in, when it is: a thread
    /// that dies where it runs keeps that time. A blocked thread was charged
    /// when it blocked, so nothing is added for it.
    pub fn charge_cpu_if_running(&mut self, now: u64) {
        if self.state == ThreadState::Running && !self.blocked_in_syscall {
            self.charge_cpu(now);
        }
    }

    /// Atomically account each dispatched interval once, including a remote
    /// scheduler read racing a syscall boundary. The timestamp's low bit is
    /// its mode, so a boundary charges the mode that preceded it.
    fn update_timer_cpu(&self, mode: Option<bool>, stop: bool) {
        let now = crate::signal::monotonic_micros();
        let stamp = &self.signals.cpu_clock;
        let mut old = stamp.load(Ordering::Acquire);
        loop {
            if old == 0 && mode.is_none() { return; }
            let user = mode.unwrap_or(old & 1 != 0);
            let next = if stop { 0 } else { (now.max(old >> 1) << 1) | u64::from(user) };
            match stamp.compare_exchange_weak(old, next, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => break,
                Err(value) => old = value,
            }
        }
        if old != 0 {
            let ran = now.saturating_sub(old >> 1).saturating_mul(1000);
            let user = old & 1 != 0;
            let own = if user { &self.signals.user_ns } else { &self.signals.system_ns };
            own.fetch_add(ran, Ordering::Relaxed);
            if let Some(account) = &self.cpu_account {
                let counter = if user { &account.user_ns } else { &account.system_ns };
                counter.fetch_add(ran, Ordering::Relaxed);
            }
        }
    }

    pub fn charge_timer_cpu(&self) { self.update_timer_cpu(None, false); }

    pub fn stop_timer_cpu(&self) { self.update_timer_cpu(None, true); }

    fn switch_timer_mode(&self, user: bool) {
        self.update_timer_cpu(Some(user), false);
        self.signals.in_user.store(user, Ordering::Relaxed);
    }

    /// Claim this thread for an immediate kill. Refused while the thread is
    /// inside a kill-custody section; the caller then defers the kill.
    pub(crate) fn claim_for_kill(&self) -> bool {
        self.kill_custody
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |word| {
                (word & CUSTODY_COUNT == 0).then_some(word | KILL_CLAIMED)
            })
            .is_ok()
    }

    /// Withdraw a claim taken by `claim_for_kill` when the kill was deferred.
    pub(crate) fn release_kill_claim(&self) {
        self.kill_custody.fetch_and(!KILL_CLAIMED, Ordering::AcqRel);
    }

    /// Whether the thread is inside a kill-custody section.
    pub(crate) fn in_kill_custody(&self) -> bool {
        self.kill_custody.load(Ordering::Acquire) & CUSTODY_COUNT != 0
    }

    /// Record that a SIGKILL left pending for this thread waits for it to
    /// leave its section. From here on the thread must not sleep in a wait
    /// the signal ends: a wake that found it still running is not repeated,
    /// so such a wait started afterwards could last for ever. The scheduler's
    /// block primitives refuse to block it (`must_not_sleep`), and its wait
    /// goes on to its signal check. Set and read under the scheduler lock, so
    /// each block either sees it or comes before the wake that follows it.
    pub(crate) fn mark_kill_pending(&self) {
        self.kill_custody.fetch_or(KILL_PENDING, Ordering::AcqRel);
    }

    /// Whether a block primitive must refuse to block this thread: a SIGKILL
    /// is pending for it and it is inside no section but its syscall's own.
    /// A nested section (an ext2 lock it holds or is queued for) is one the
    /// kill lets it finish; its waits end when the lock is released, and the
    /// holder they wait for may need this CPU, so they still sleep.
    pub(crate) fn must_not_sleep(&self) -> bool {
        let word = self.kill_custody.load(Ordering::Acquire);
        word & KILL_PENDING != 0 && word & CUSTODY_COUNT <= 1
    }

    /// Whether a SIGKILL was left pending for this thread while it was inside
    /// a section (`mark_kill_pending`).
    #[cfg(target_arch = "x86_64")]
    pub(crate) fn kill_pending(&self) -> bool {
        self.kill_custody.load(Ordering::Acquire) & KILL_PENDING != 0
    }

    /// Close every section the thread's syscall had open, for a syscall whose
    /// kernel stack is discarded instead of unwound: x86-64 returns a pause or
    /// sigsuspend that a signal ends to user mode straight from the context
    /// switch, so the syscall's `KillCustody` guards are never dropped. Left
    /// open, they would refuse every later kill claim and make every later
    /// syscall look nested.
    #[cfg(target_arch = "x86_64")]
    pub(crate) fn abandon_syscall_custody(&self) {
        self.kill_custody.fetch_and(!CUSTODY_COUNT, Ordering::AcqRel);
    }
}

/// A kernel section the running thread must be allowed to finish before a
/// SIGKILL takes it: it holds, or is queued for, a lock other threads need.
/// While one is open, `kill_process_now` leaves SIGKILL pending instead of
/// terminating the thread, and the thread dies at its return to user mode
/// with the lock released. A thread already claimed by a kill enters no
/// section, and must not take the lock either: see `try_enter`.
pub struct KillCustody(Option<&'static AtomicU64>);

impl KillCustody {
    /// Open a section for the running thread. `None` means a kill has claimed
    /// it: its termination is imminent and would discard anything it then
    /// acquired, so the caller must wait for the termination, or for the
    /// claim to be withdrawn, and try again. With no current thread there is
    /// nothing a kill could claim, and the guard is empty.
    pub fn try_enter() -> Option<Self> {
        #[cfg(target_arch = "x86_64")]
        let thread = crate::per_cpu::current_thread();
        #[cfg(target_arch = "aarch64")]
        let thread = crate::per_cpu_aarch64::current_thread();
        let Some(thread) = thread else {
            return Some(KillCustody(None));
        };
        let thread: &'static Thread = thread;
        let word = &thread.kill_custody;
        word.fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count & KILL_CLAIMED == 0).then_some(count + 1)
        })
        .ok()
        .map(|_| KillCustody(Some(word)))
    }

    /// Open the section a syscall runs in, from syscall entry to the start of
    /// its return to user mode. Everything a syscall owns lives on its kernel
    /// stack, which a termination discards without running a destructor, so a
    /// SIGKILL never takes a thread inside one: it stays pending, wakes the
    /// thread's wait, and ends the process at the syscall's return, once the
    /// stack has been unwound. A thread a kill claimed while it was in user
    /// mode has not started the syscall and owns nothing yet. It waits,
    /// preemptible, for the termination to switch it away, or for the claim to
    /// be withdrawn. Called with the syscall's preempt_disable() in force.
    pub fn enter_syscall() -> Self {
        loop {
            if let Some(custody) = Self::try_enter() {
                if let Some(thread) = current_cpu_thread() {
                    thread.switch_timer_mode(false);
                }
                return custody;
            }
            crate::per_cpu::preempt_enable();
            crate::arch_halt_with_interrupts();
            crate::per_cpu::preempt_disable();
        }
    }
}

/// Charge the calling thread's CPU time to user mode from here on, if it was
/// being charged to system mode. The return to user mode calls this last,
/// after the signal check and delivery, so the work of returning is system
/// time; `KillCustody::enter_syscall` started charging system time.
pub fn resume_user_time() {
    if let Some(thread) = current_cpu_thread() {
        if !thread.signals.in_user.load(Ordering::Relaxed) {
            thread.switch_timer_mode(true);
        }
    }
}

/// Charge the calling thread's CPU time to system mode from here on, if it was
/// being charged to user mode: an exception taken from user mode.
pub fn enter_kernel_time() {
    if let Some(thread) = current_cpu_thread() {
        if thread.signals.in_user.load(Ordering::Relaxed) {
            thread.switch_timer_mode(false);
        }
    }
}

/// The calling thread's CPU time in nanoseconds, its own or, with `process`,
/// its process's, charged up to now first. Lock-free, for the CPU-time
/// clocks: other threads of the process add their open run intervals at their
/// next kernel entry, exit or scheduler tick. None from a kernel thread.
pub fn current_cpu_time_ns(process: bool) -> Option<u64> {
    let thread = current_cpu_thread()?;
    thread.charge_timer_cpu();
    let (user, system) = if process {
        thread.cpu_account.as_ref()?.split_ns()
    } else {
        thread.signals.cpu_split_ns()
    };
    Some(user.saturating_add(system))
}

/// Charge the calling thread's open run interval, so its own and its
/// process's CPU time read next include it.
pub fn charge_current_cpu_time() {
    if let Some(thread) = current_cpu_thread() {
        thread.charge_timer_cpu();
    }
}

/// The calling thread's own user and system nanoseconds, charged up to now
/// first. None from a kernel thread.
pub fn current_thread_cpu_split_ns() -> Option<(u64, u64)> {
    let thread = current_cpu_thread()?;
    thread.charge_timer_cpu();
    Some(thread.signals.cpu_split_ns())
}

impl Drop for KillCustody {
    fn drop(&mut self) {
        if let Some(word) = self.0 {
            word.fetch_sub(1, Ordering::Release);
        }
    }
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
            // Kernel stacks cannot be cloned. Use `publish_to_scheduler` when
            // publishing a process-table row so ownership moves to the copy.
            kernel_stack_allocation: None,
            #[cfg(target_arch = "x86_64")]
            fpu: self.fpu,
            tls_block: self.tls_block,
            priority: self.priority,
            time_slice: self.time_slice,
            entry_point: self.entry_point, // fn pointers can be copied
            privilege: self.privilege,
            has_started: self.has_started,
            blocked_in_syscall: self.blocked_in_syscall,
            saved_by_inline_schedule: false,
            inline_schedule_spsr: 0,
            inline_schedule_prev_elr: 0,
            inline_schedule_caller_lr: self.inline_schedule_caller_lr,
            inline_schedule_saved_sp: self.inline_schedule_saved_sp,
            saved_userspace_context: self.saved_userspace_context.clone(),
            wake_time_ns: self.wake_time_ns,
            realtime_sleep: false,
            timer_pop: self.timer_pop,
            run_start_ticks: self.run_start_ticks,
            cpu_ticks_total: self.cpu_ticks_total,
            resource_limits: self.resource_limits.clone(),
            cpu_account: self.cpu_account.clone(),
            signals: self.signals.clone(),
            signal_timers: self.signal_timers.clone(),
            owner_pid: self.owner_pid,
            cached_ttbr0: self.cached_ttbr0,
            // Carried, not reset: `publish_to_scheduler` clones a process-table
            // row into the scheduler for the SAME thread, and the dispatch mark
            // stamped before that publish is compared against the value after
            // it. Resetting here would make the next identical-frame save read
            // as a backwards jump.
            wait_loop_iters: AtomicU64::new(self.wait_loop_iters.load(Ordering::Relaxed)),
            kill_custody: AtomicU64::new(self.kill_custody.load(Ordering::Acquire)),
            cpu_affinity: self.cpu_affinity,
        }
    }
}

impl Thread {
    /// Clear the classification and diagnostics owned by an inline scheduler
    /// save. Call this whenever a fresh context replaces that saved context.
    ///
    /// The fields this clears are arch-neutral on `Thread` (aarch64 is the only
    /// arch that ever sets them — the ret-based inline-schedule dispatch this
    /// tracks has no x86_64 analogue, which always resumes via IRETQ), so this
    /// stays a no-op on x86_64 rather than a wrong one: #721 generalized its one
    /// caller, `ExecSchedCommit::apply`, off its original aarch64-only gate, and
    /// an exec'd thread's context was never saved by an inline schedule on
    /// either arch.
    pub(crate) fn clear_inline_schedule_state(&mut self) {
        self.saved_by_inline_schedule = false;
        self.inline_schedule_spsr = 0;
        self.inline_schedule_prev_elr = 0;
        self.inline_schedule_saved_sp = 0;
        self.inline_schedule_caller_lr = 0;
    }

    /// Produce the scheduler's publication copy of a process-table row thread,
    /// moving sole ownership of the kernel-stack allocation to that copy.
    ///
    /// `Thread::clone` cannot clone a `KernelStack`, so a naive publish would drop
    /// the allocation. Ownership therefore MOVES: the scheduler copy is the single
    /// owner and is freed only behind the scheduler's two-epoch retirement grace;
    /// the row's copy is left holding `None` because ownership moved, not because
    /// it leaked.
    pub fn publish_to_scheduler(&mut self) -> Thread {
        let mut published = self.clone();
        published.kernel_stack_allocation = self.kernel_stack_allocation.take();
        if let Some(allocation) = published.kernel_stack_allocation.as_mut() {
            allocation.set_owner_pid(self.owner_pid);
        }
        let ownership = crate::memory::kernel_stack::classify_kernel_stack_ownership(
            published.kernel_stack_top.map(|top| top.as_u64()),
            published.kernel_stack_allocation.is_some(),
            self.kernel_stack_allocation.is_some(),
        );
        crate::memory::kernel_stack::note_publication(ownership);
        published
    }

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
            #[cfg(target_arch = "x86_64")]
            fpu: crate::arch_impl::x86_64::fpu::FpuState::initial(),
            tls_block,
            priority: 64,      // Higher priority for kernel threads
            time_slice: 20,    // Longer time slice
            entry_point: None, // Kernel threads use direct entry
            privilege: ThreadPrivilege::Kernel,
            has_started: false,        // New thread hasn't run yet
            blocked_in_syscall: false, // New thread is not blocked in syscall
            saved_by_inline_schedule: false,
            inline_schedule_spsr: 0,
            inline_schedule_prev_elr: 0,
            inline_schedule_caller_lr: 0,
            inline_schedule_saved_sp: 0,
            saved_userspace_context: None,
            wake_time_ns: None,
            realtime_sleep: false,
            timer_pop: None,
            run_start_ticks: 0,
            cpu_ticks_total: 0,
            resource_limits: None,
            cpu_account: None,
            signals: alloc::sync::Arc::new(crate::signal::ThreadSignals::with_mask(0)),
            signal_timers: None,
            owner_pid: None,
            cached_ttbr0: 0,
            wait_loop_iters: core::sync::atomic::AtomicU64::new(0),
            kill_custody: core::sync::atomic::AtomicU64::new(0),
            cpu_affinity: None,
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
            #[cfg(target_arch = "x86_64")]
            fpu: crate::arch_impl::x86_64::fpu::FpuState::initial(),
            tls_block,
            priority: 64,
            time_slice: 20,
            entry_point: None,
            privilege: ThreadPrivilege::Kernel,
            has_started: false,
            blocked_in_syscall: false,
            saved_by_inline_schedule: false,
            inline_schedule_spsr: 0,
            inline_schedule_prev_elr: 0,
            inline_schedule_caller_lr: 0,
            inline_schedule_saved_sp: 0,
            saved_userspace_context: None,
            wake_time_ns: None,
            realtime_sleep: false,
            timer_pop: None,
            run_start_ticks: 0,
            cpu_ticks_total: 0,
            resource_limits: None,
            cpu_account: None,
            signals: alloc::sync::Arc::new(crate::signal::ThreadSignals::with_mask(0)),
            signal_timers: None,
            owner_pid: None,
            cached_ttbr0: 0,
            wait_loop_iters: core::sync::atomic::AtomicU64::new(0),
            kill_custody: core::sync::atomic::AtomicU64::new(0),
            cpu_affinity: None,
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
            #[cfg(target_arch = "x86_64")]
            fpu: crate::arch_impl::x86_64::fpu::FpuState::initial(),
            tls_block,
            priority: 128,  // Default medium priority
            time_slice: 10, // Default time slice
            entry_point: Some(entry_point),
            privilege,
            has_started: false,        // New thread hasn't run yet
            blocked_in_syscall: false, // New thread is not blocked in syscall
            saved_by_inline_schedule: false,
            inline_schedule_spsr: 0,
            inline_schedule_prev_elr: 0,
            inline_schedule_caller_lr: 0,
            inline_schedule_saved_sp: 0,
            saved_userspace_context: None,
            wake_time_ns: None,
            realtime_sleep: false,
            timer_pop: None,
            run_start_ticks: 0,
            cpu_ticks_total: 0,
            resource_limits: None,
            cpu_account: None,
            signals: alloc::sync::Arc::new(crate::signal::ThreadSignals::with_mask(0)),
            signal_timers: None,
            owner_pid: None,
            cached_ttbr0: 0,
            wait_loop_iters: core::sync::atomic::AtomicU64::new(0),
            kill_custody: core::sync::atomic::AtomicU64::new(0),
            cpu_affinity: None,
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
        let context = CpuContext::new(VirtAddr::new(entry_point as u64), stack_top, privilege);

        Self {
            id,
            name,
            state: ThreadState::Ready,
            context,
            stack_top,
            stack_bottom,
            kernel_stack_top: None,
            kernel_stack_allocation: None,
            #[cfg(target_arch = "x86_64")]
            fpu: crate::arch_impl::x86_64::fpu::FpuState::initial(),
            tls_block,
            priority: 128,
            time_slice: 10,
            entry_point: Some(entry_point),
            privilege,
            has_started: false,
            blocked_in_syscall: false,
            saved_by_inline_schedule: false,
            inline_schedule_spsr: 0,
            inline_schedule_prev_elr: 0,
            inline_schedule_caller_lr: 0,
            inline_schedule_saved_sp: 0,
            saved_userspace_context: None,
            wake_time_ns: None,
            realtime_sleep: false,
            timer_pop: None,
            run_start_ticks: 0,
            cpu_ticks_total: 0,
            resource_limits: None,
            cpu_account: None,
            signals: alloc::sync::Arc::new(crate::signal::ThreadSignals::with_mask(0)),
            signal_timers: None,
            owner_pid: None,
            cached_ttbr0: 0,
            wait_loop_iters: core::sync::atomic::AtomicU64::new(0),
            kill_custody: core::sync::atomic::AtomicU64::new(0),
            cpu_affinity: None,
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
            kernel_stack_top: None,        // Will be set separately
            kernel_stack_allocation: None, // Will be set separately for userspace threads
            #[cfg(target_arch = "x86_64")]
            fpu: crate::arch_impl::x86_64::fpu::FpuState::initial(),
            tls_block: actual_tls_block,
            priority: 128,     // Default medium priority
            time_slice: 10,    // Default time slice
            entry_point: None, // Userspace threads don't have kernel entry points
            privilege: ThreadPrivilege::User,
            has_started: false,        // New thread hasn't run yet
            blocked_in_syscall: false, // New thread is not blocked in syscall
            saved_by_inline_schedule: false,
            inline_schedule_spsr: 0,
            inline_schedule_prev_elr: 0,
            inline_schedule_caller_lr: 0,
            inline_schedule_saved_sp: 0,
            saved_userspace_context: None,
            wake_time_ns: None,
            realtime_sleep: false,
            timer_pop: None,
            run_start_ticks: 0,
            cpu_ticks_total: 0,
            resource_limits: None,
            cpu_account: None,
            signals: alloc::sync::Arc::new(crate::signal::ThreadSignals::with_mask(0)),
            signal_timers: None,
            owner_pid: None,
            cached_ttbr0: 0,
            wait_loop_iters: core::sync::atomic::AtomicU64::new(0),
            kill_custody: core::sync::atomic::AtomicU64::new(0),
            cpu_affinity: None,
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
            #[cfg(target_arch = "x86_64")]
            fpu: crate::arch_impl::x86_64::fpu::FpuState::initial(),
            tls_block: actual_tls_block,
            priority: 128,
            time_slice: 10,
            entry_point: None,
            privilege: ThreadPrivilege::User,
            has_started: false,
            blocked_in_syscall: false,
            saved_by_inline_schedule: false,
            inline_schedule_spsr: 0,
            inline_schedule_prev_elr: 0,
            inline_schedule_caller_lr: 0,
            inline_schedule_saved_sp: 0,
            saved_userspace_context: None,
            wake_time_ns: None,
            realtime_sleep: false,
            timer_pop: None,
            run_start_ticks: 0,
            cpu_ticks_total: 0,
            resource_limits: None,
            cpu_account: None,
            signals: alloc::sync::Arc::new(crate::signal::ThreadSignals::with_mask(0)),
            signal_timers: None,
            owner_pid: None,
            cached_ttbr0: 0,
            wait_loop_iters: core::sync::atomic::AtomicU64::new(0),
            kill_custody: core::sync::atomic::AtomicU64::new(0),
            cpu_affinity: None,
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
        self.signals.cpu_clock.store(
            (crate::signal::monotonic_micros() << 1) | u64::from(self.signals.in_user.load(Ordering::Relaxed)),
            Ordering::Release,
        );
    }

    /// Mark thread as ready
    pub fn set_ready(&mut self) {
        self.stop_timer_cpu();
        if self.state != ThreadState::Terminated {
            self.state = ThreadState::Ready;
        }
    }

    /// Mark thread as terminated
    pub fn set_terminated(&mut self) {
        self.stop_timer_cpu();
        self.realtime_sleep = false;
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
            #[cfg(target_arch = "x86_64")]
            fpu: crate::arch_impl::x86_64::fpu::FpuState::initial(),
            tls_block,
            priority: 128,  // Default medium priority
            time_slice: 10, // Default time slice
            entry_point: Some(entry_point),
            privilege,
            has_started: false,        // New thread hasn't run yet
            blocked_in_syscall: false, // New thread is not blocked in syscall
            saved_by_inline_schedule: false,
            inline_schedule_spsr: 0,
            inline_schedule_prev_elr: 0,
            inline_schedule_caller_lr: 0,
            inline_schedule_saved_sp: 0,
            saved_userspace_context: None,
            wake_time_ns: None,
            realtime_sleep: false,
            timer_pop: None,
            run_start_ticks: 0,
            cpu_ticks_total: 0,
            resource_limits: None,
            cpu_account: None,
            signals: alloc::sync::Arc::new(crate::signal::ThreadSignals::with_mask(0)),
            signal_timers: None,
            owner_pid: None,
            cached_ttbr0: 0,
            wait_loop_iters: core::sync::atomic::AtomicU64::new(0),
            kill_custody: core::sync::atomic::AtomicU64::new(0),
            cpu_affinity: None,
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
        let context = CpuContext::new(VirtAddr::new(entry_point as u64), stack_top, privilege);

        Self {
            id,
            name,
            state: ThreadState::Ready,
            context,
            stack_top,
            stack_bottom,
            kernel_stack_top: None,
            kernel_stack_allocation: None,
            #[cfg(target_arch = "x86_64")]
            fpu: crate::arch_impl::x86_64::fpu::FpuState::initial(),
            tls_block,
            priority: 128,
            time_slice: 10,
            entry_point: Some(entry_point),
            privilege,
            has_started: false,
            blocked_in_syscall: false,
            saved_by_inline_schedule: false,
            inline_schedule_spsr: 0,
            inline_schedule_prev_elr: 0,
            inline_schedule_caller_lr: 0,
            inline_schedule_saved_sp: 0,
            saved_userspace_context: None,
            wake_time_ns: None,
            realtime_sleep: false,
            timer_pop: None,
            run_start_ticks: 0,
            cpu_ticks_total: 0,
            resource_limits: None,
            cpu_account: None,
            signals: alloc::sync::Arc::new(crate::signal::ThreadSignals::with_mask(0)),
            signal_timers: None,
            owner_pid: None,
            cached_ttbr0: 0,
            wait_loop_iters: core::sync::atomic::AtomicU64::new(0),
            kill_custody: core::sync::atomic::AtomicU64::new(0),
            cpu_affinity: None,
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
    let entry_point = crate::per_cpu::current_thread().and_then(|t| t.entry_point.take());

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

fn current_cpu_thread() -> Option<&'static Thread> {
    #[cfg(target_arch = "x86_64")]
    let thread = crate::per_cpu::current_thread();
    #[cfg(target_arch = "aarch64")]
    let thread = crate::per_cpu_aarch64::current_thread();
    thread.map(|t| &*t)
}
