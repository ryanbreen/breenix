//! ARM64 SMP (Symmetric Multi-Processing) support.
//!
//! This module handles bringing up secondary CPUs on ARM64 using PSCI
//! (Power State Coordination Interface). QEMU's virt machine uses PSCI
//! with HVC calls to power on secondary CPUs.
//!
//! Flow:
//! 1. CPU 0 calls `release_cpu()` which issues PSCI CPU_ON via HVC
//! 2. PSCI firmware starts the target CPU at `secondary_cpu_entry` (boot.S)
//! 3. boot.S sets up stack, MMU, and calls `secondary_cpu_entry_rust()`
//! 4. Rust entry marks the CPU online and enters WFI loop (Phase 1)

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Maximum number of CPUs supported.
pub const MAX_CPUS: usize = 8;

/// PSCI function IDs (SMCCC compliant, 64-bit).
const PSCI_CPU_ON_64: u64 = 0xC400_0003;

extern "C" {
    /// Physical address of secondary_cpu_entry, stored in .rodata by boot.S.
    /// We cannot reference secondary_cpu_entry directly from Rust because it lives
    /// in .text.boot (low physical memory) while Rust code is in high-half virtual
    /// memory — the ~1 TiB gap exceeds the ADRP relocation range (+/- 4 GiB).
    static SECONDARY_CPU_ENTRY_PHYS: u64;
}

/// Number of CPUs currently online (starts at 1 for the boot CPU).
static CPUS_ONLINE: AtomicU64 = AtomicU64::new(1);

/// Per-CPU online status flags.
static CPU_ONLINE: [AtomicBool; MAX_CPUS] = [
    AtomicBool::new(true),  // CPU 0 is online at boot
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
];

/// Issue a PSCI CPU_ON call via HVC to start a secondary CPU.
///
/// Arguments:
/// - `target_cpu`: MPIDR of the target CPU (Aff0 = cpu_id for QEMU virt)
/// - `entry_point`: Physical address where the CPU starts executing
/// - `context_id`: Value passed in x0 to the new CPU (we use cpu_id)
///
/// Returns PSCI status: 0 = SUCCESS, negative = error.
fn psci_cpu_on(target_cpu: u64, entry_point: u64, context_id: u64) -> i64 {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "hvc #0",
            inout("x0") PSCI_CPU_ON_64 => ret,
            in("x1") target_cpu,
            in("x2") entry_point,
            in("x3") context_id,
            options(nomem, nostack),
        );
    }
    ret
}

/// Release a secondary CPU using PSCI CPU_ON.
///
/// The CPU will start executing at `secondary_cpu_entry` in boot.S,
/// which sets up the stack and MMU, then calls `secondary_cpu_entry_rust(cpu_id)`.
pub fn release_cpu(cpu_id: usize) {
    if cpu_id == 0 || cpu_id >= MAX_CPUS {
        return;
    }

    // Get the physical address of the secondary entry point in boot.S
    let entry_phys = unsafe { core::ptr::read_volatile(&SECONDARY_CPU_ENTRY_PHYS) };

    // MPIDR for QEMU virt: Aff0 = cpu_id, all other affinity fields = 0
    let target_mpidr = cpu_id as u64;

    // Context ID: pass cpu_id so the new CPU knows who it is
    let context_id = cpu_id as u64;

    let ret = psci_cpu_on(target_mpidr, entry_phys, context_id);

    if ret != 0 {
        // PSCI error — emit raw UART error indicator
        raw_uart_char(b'E');
        raw_uart_char(b'0' + cpu_id as u8);
    }
}

/// Get the number of CPUs currently online.
pub fn cpus_online() -> u64 {
    CPUS_ONLINE.load(Ordering::Acquire)
}

/// Check if a specific CPU is online.
#[allow(dead_code)]
pub fn is_cpu_online(cpu_id: usize) -> bool {
    if cpu_id >= MAX_CPUS {
        return false;
    }
    CPU_ONLINE[cpu_id].load(Ordering::Acquire)
}

/// Raw UART output for secondary CPUs (no locks, no allocations).
#[inline(always)]
fn raw_uart_char(c: u8) {
    const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
    const UART_VIRT: u64 = HHDM_BASE + 0x0900_0000;
    unsafe {
        let ptr = UART_VIRT as *mut u8;
        core::ptr::write_volatile(ptr, c);
    }
}

/// Secondary CPU entry point.
///
/// Called from boot.S after PSCI CPU_ON starts the CPU and boot.S
/// sets up the stack, MMU, and exception vectors.
///
/// Initializes per-CPU data, GIC CPU interface, timer, and creates
/// this CPU's idle thread. After initialization, enters the idle loop
/// and participates in scheduling.
#[no_mangle]
pub extern "C" fn secondary_cpu_entry_rust(cpu_id: u64) -> ! {
    // Emit raw UART character to signal this CPU is alive
    raw_uart_char(b'0' + cpu_id as u8);

    // Initialize per-CPU data (sets TPIDR_EL1 for this CPU)
    crate::per_cpu_aarch64::init_cpu(cpu_id as usize);

    // Set kernel stack top for this CPU.
    // boot.S sets SP to 0x41000000 + (cpu_id+1)*0x200000 (physical),
    // then adds KERNEL_VIRT_BASE after enabling MMU.
    // This value is critical: when a user thread runs on this CPU and an
    // exception occurs, the kernel needs to switch to this stack.
    const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
    const STACK_REGION_BASE: u64 = 0x4100_0000;
    const STACK_SIZE: u64 = 0x20_0000; // 2MB per CPU
    let kernel_stack_top = HHDM_BASE + STACK_REGION_BASE + (cpu_id + 1) * STACK_SIZE;
    crate::per_cpu_aarch64::set_kernel_stack_top(kernel_stack_top);

    // Initialize GIC CPU interface (GICC registers are banked per-CPU)
    super::gic::init_cpu_interface_secondary();

    // Initialize timer for this CPU (arm virtual timer, enable PPI 27)
    super::timer_interrupt::init_secondary();

    // Create and register this CPU's idle thread with the scheduler.
    // This must happen before enabling interrupts — the scheduler needs
    // an idle thread for this CPU before timer interrupts fire.
    create_and_register_idle_thread(cpu_id as usize);

    // Enable interrupts so this CPU can handle timer ticks
    unsafe {
        super::cpu::enable_interrupts();
    }

    // Mark this CPU as online (after all init is complete)
    if (cpu_id as usize) < MAX_CPUS {
        CPU_ONLINE[cpu_id as usize].store(true, Ordering::Release);
    }
    CPUS_ONLINE.fetch_add(1, Ordering::Release);

    // Idle loop — wait for interrupts, handle timer, participate in scheduling
    loop {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }
}

/// Create an idle thread for a secondary CPU and register it with the scheduler.
fn create_and_register_idle_thread(cpu_id: usize) {
    use alloc::boxed::Box;
    use alloc::format;
    use crate::task::thread::{Thread, ThreadState, ThreadPrivilege};
    use crate::memory::arch_stub::VirtAddr;

    // Dummy stack addresses (the CPU is already running on its boot stack)
    let dummy_stack_top = VirtAddr::new(0x4000_0000 + (cpu_id as u64) * 0x20_0000);
    let dummy_stack_bottom = VirtAddr::new(0x4000_0000 + (cpu_id as u64 - 1) * 0x20_0000);
    let dummy_tls = VirtAddr::zero();

    let mut idle_task = Box::new(Thread::new(
        format!("swapper/{}", cpu_id),
        idle_thread_fn,
        dummy_stack_top,
        dummy_stack_bottom,
        dummy_tls,
        ThreadPrivilege::Kernel,
    ));

    // Mark as running and already started (this CPU is already executing)
    idle_task.state = ThreadState::Running;
    idle_task.has_started = true;

    // Set per-CPU current thread pointer
    let idle_task_ptr = &*idle_task as *const _ as *mut crate::task::thread::Thread;
    crate::per_cpu_aarch64::set_current_thread(idle_task_ptr);

    // Register with the global scheduler
    crate::task::scheduler::register_cpu_idle_thread(cpu_id, idle_task);
}

/// Idle thread function for secondary CPUs.
fn idle_thread_fn() {
    loop {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }
}
