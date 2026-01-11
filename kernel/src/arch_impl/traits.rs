//! Architecture-agnostic traits for hardware abstraction.
//!
//! These traits define the interface between architecture-specific code and
//! the rest of the kernel. Each architecture must implement these traits.

use core::ops::BitOr;

/// CPU privilege level abstraction.
///
/// Maps to Ring 0/3 on x86_64 and EL0/EL1 on ARM64.
pub trait PrivilegeLevel: Copy + Eq {
    /// Returns the kernel privilege level (Ring 0 / EL1).
    fn kernel() -> Self;

    /// Returns the user privilege level (Ring 3 / EL0).
    fn user() -> Self;

    /// Returns true if this is kernel privilege level.
    fn is_kernel(&self) -> bool;

    /// Returns true if this is user privilege level.
    fn is_user(&self) -> bool;
}

/// Interrupt/exception frame abstraction.
///
/// Represents the CPU state saved when an interrupt or exception occurs.
/// The exact layout varies by architecture.
pub trait InterruptFrame {
    /// The architecture's privilege level type.
    type Privilege: PrivilegeLevel;

    /// Returns the instruction pointer (RIP on x86, PC on ARM).
    fn instruction_pointer(&self) -> u64;

    /// Returns the stack pointer (RSP on x86, SP on ARM).
    fn stack_pointer(&self) -> u64;

    /// Sets the instruction pointer.
    fn set_instruction_pointer(&mut self, addr: u64);

    /// Sets the stack pointer.
    fn set_stack_pointer(&mut self, addr: u64);

    /// Returns the privilege level at the time of the interrupt.
    fn privilege_level(&self) -> Self::Privilege;

    /// Returns true if the interrupt came from userspace.
    #[inline]
    fn is_from_userspace(&self) -> bool {
        self.privilege_level().is_user()
    }
}

/// Page table flag operations.
///
/// Abstracts over architecture-specific page table entry flags.
pub trait PageFlags: Copy + Clone + BitOr<Output = Self> + Sized {
    /// No flags set (empty).
    fn empty() -> Self;

    /// Page is present/valid.
    fn present() -> Self;

    /// Page is writable.
    fn writable() -> Self;

    /// Page is accessible from userspace.
    fn user_accessible() -> Self;

    /// Page is not executable (NX/XN bit).
    fn no_execute() -> Self;

    /// OS-available bit used for Copy-on-Write marking.
    fn cow_marker() -> Self;

    /// Page should not be cached (for MMIO).
    fn no_cache() -> Self;

    /// Check if a specific flag is set.
    fn contains(&self, other: Self) -> bool;
}

/// Page table operations.
///
/// Provides low-level page table manipulation primitives.
pub trait PageTableOps {
    /// The architecture's page flags type.
    type Flags: PageFlags;

    /// Read the current page table root address (CR3 on x86, TTBR on ARM).
    fn read_root() -> u64;

    /// Write a new page table root address.
    ///
    /// # Safety
    ///
    /// The address must point to a valid page table structure.
    unsafe fn write_root(addr: u64);

    /// Flush a single TLB entry for the given virtual address.
    fn flush_tlb_page(addr: u64);

    /// Flush the entire TLB.
    fn flush_tlb_all();

    /// Number of page table levels (4 for x86_64, varies for ARM).
    const PAGE_LEVELS: usize;

    /// Page size in bytes (typically 4096).
    const PAGE_SIZE: usize;

    /// Number of entries per page table (512 for x86_64 with 4KB pages).
    const ENTRIES_PER_TABLE: usize;
}

/// Per-CPU data access operations.
///
/// Abstracts over architecture-specific per-CPU data access mechanisms
/// (GS segment on x86_64, TPIDR_EL1 on ARM64).
pub trait PerCpuOps {
    /// Get the current CPU's ID.
    fn cpu_id() -> u64;

    /// Get the current thread pointer.
    ///
    /// Returns null if no thread is currently assigned.
    fn current_thread_ptr() -> *mut u8;

    /// Set the current thread pointer.
    ///
    /// # Safety
    ///
    /// The pointer must be valid for the lifetime it will be used.
    unsafe fn set_current_thread_ptr(ptr: *mut u8);

    /// Get the kernel stack top for this CPU.
    fn kernel_stack_top() -> u64;

    /// Set the kernel stack top for this CPU.
    ///
    /// # Safety
    ///
    /// The address must point to a valid kernel stack.
    unsafe fn set_kernel_stack_top(addr: u64);

    /// Get the current preemption count.
    fn preempt_count() -> u32;

    /// Disable preemption (increment preempt count).
    fn preempt_disable();

    /// Enable preemption (decrement preempt count).
    ///
    /// May trigger rescheduling if count reaches zero and reschedule is needed.
    fn preempt_enable();

    /// Check if currently in any interrupt context.
    fn in_interrupt() -> bool;

    /// Check if currently in hard IRQ context.
    fn in_hardirq() -> bool;

    /// Check if scheduling is allowed (preempt count is zero).
    fn can_schedule() -> bool;
}

/// Syscall frame abstraction.
///
/// Provides access to syscall arguments in an architecture-independent way.
/// The actual register mapping varies by architecture:
/// - x86_64: RAX=number, RDI/RSI/RDX/R10/R8/R9=args
/// - ARM64: X8=number, X0-X5=args
pub trait SyscallFrame {
    /// Get the syscall number.
    fn syscall_number(&self) -> u64;

    /// Get syscall argument 1.
    fn arg1(&self) -> u64;

    /// Get syscall argument 2.
    fn arg2(&self) -> u64;

    /// Get syscall argument 3.
    fn arg3(&self) -> u64;

    /// Get syscall argument 4.
    fn arg4(&self) -> u64;

    /// Get syscall argument 5.
    fn arg5(&self) -> u64;

    /// Get syscall argument 6.
    fn arg6(&self) -> u64;

    /// Set the return value for the syscall.
    fn set_return_value(&mut self, value: u64);

    /// Get the return value (for inspection).
    fn return_value(&self) -> u64;
}

/// Timer and timestamp operations.
///
/// Abstracts over architecture-specific high-resolution timers
/// (TSC on x86_64, generic timer on ARM64).
pub trait TimerOps {
    /// Read the current timestamp counter value.
    fn read_timestamp() -> u64;

    /// Get the timer frequency in Hz.
    ///
    /// Returns None if the frequency hasn't been calibrated yet.
    fn frequency_hz() -> Option<u64>;

    /// Convert timestamp ticks to nanoseconds.
    fn ticks_to_nanos(ticks: u64) -> u64;
}

/// Interrupt controller operations.
///
/// Abstracts over architecture-specific interrupt controllers
/// (PIC/APIC on x86, GIC on ARM).
pub trait InterruptController {
    /// Initialize the interrupt controller.
    fn init();

    /// Enable an IRQ line.
    fn enable_irq(irq: u8);

    /// Disable an IRQ line.
    fn disable_irq(irq: u8);

    /// Send End-of-Interrupt signal for a vector.
    fn send_eoi(vector: u8);

    /// Get the IRQ offset (base vector number for hardware IRQs).
    fn irq_offset() -> u8;
}

/// Basic CPU control operations.
pub trait CpuOps {
    /// Enable interrupts.
    ///
    /// # Safety
    ///
    /// Must be called in appropriate context where interrupts can be safely enabled.
    unsafe fn enable_interrupts();

    /// Disable interrupts.
    ///
    /// # Safety
    ///
    /// Must be called in appropriate context.
    unsafe fn disable_interrupts();

    /// Check if interrupts are currently enabled.
    fn interrupts_enabled() -> bool;

    /// Halt the CPU until the next interrupt.
    fn halt();

    /// Halt with interrupts enabled (wait for interrupt).
    fn halt_with_interrupts();
}
