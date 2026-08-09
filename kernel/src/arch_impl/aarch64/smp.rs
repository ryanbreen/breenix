//! ARM64 SMP (Symmetric Multi-Processing) support.
//!
//! This module handles bringing up secondary CPUs on ARM64 using PSCI
//! (Power State Coordination Interface). All supported hypervisors
//! (QEMU, Parallels, VMware) implement PSCI via HVC calls.
//!
//! Flow:
//! 1. CPU 0 probes CPUs 1..MAX_CPUS via `release_cpu()` (PSCI CPU_ON)
//! 2. PSCI firmware starts each target CPU at `secondary_cpu_entry` (boot.S)
//! 3. boot.S sets up stack, MMU, and calls `secondary_cpu_entry_rust()`
//! 4. Rust entry initializes per-CPU data, GIC, timer, creates idle thread

use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};

/// Maximum number of CPUs supported.
pub const MAX_CPUS: usize = 8;

pub const CNTVCT_FALLBACK_FREQUENCY_HZ: u64 = 1_000_000;
const PSCI_CPU_ON_MAX_ATTEMPTS: usize = 4;
const PSCI_CPU_ON_RETRY_BACKOFF_MICROSECONDS: u64 = 500;
const PSCI_CPU_ON_BACKOFF_ITERATION_CAP: usize = 1_000_000;
const PSCI_RETURN_SUCCESS: i64 = 0;
const PSCI_RETURN_ALREADY_ON: i64 = -4;
const PSCI_RETURN_ON_PENDING: i64 = -5;
const PSCI_RETURN_INTERNAL_FAILURE: i64 = -6;
const PSCI_RETURN_NOT_ATTEMPTED: i64 = i64::MIN;

static LAST_PSCI_RETURN_CODE: [AtomicI64; MAX_CPUS] =
    [const { AtomicI64::new(PSCI_RETURN_NOT_ATTEMPTED) }; MAX_CPUS];

/// PSCI function IDs (SMCCC compliant).
const PSCI_CPU_ON_64: u64 = 0xC400_0003;
const PSCI_CPU_ON_32: u64 = 0x8400_0003;

extern "C" {
    /// Physical address of secondary_cpu_entry, stored in .rodata by boot.S.
    /// We cannot reference secondary_cpu_entry directly from Rust because it lives
    /// in .text.boot (low physical memory) while Rust code is in high-half virtual
    /// memory — the ~1 TiB gap exceeds the ADRP relocation range (+/- 4 GiB).
    static SECONDARY_CPU_ENTRY_PHYS: u64;

    /// Pointer to SMP_UART_PHYS (which lives in .bss.boot, low physical memory).
    /// Stored in .rodata so Rust can reach it via ADRP, then we dereference to
    /// get the actual address of the variable and write through it.
    static SMP_UART_PHYS_PTR: u64;

    /// Pointers to SMP_TTBR0_PHYS / SMP_TTBR1_PHYS / SMP_MAIR_PHYS / SMP_TCR_PHYS
    /// variables (in .bss.boot). Secondary CPUs read these to get the correct
    /// page table addresses and MMU configuration.
    static SMP_TTBR0_PTR: u64;
    static SMP_TTBR1_PTR: u64;
    static SMP_MAIR_PTR: u64;
    static SMP_TCR_PTR: u64;

    /// Pointer to SMP_STACK_BASE_PHYS (in .bss.boot). CPU 0 writes the
    /// physical base address of the per-CPU stack region here before PSCI CPU_ON.
    /// On QEMU/Parallels: 0x4300_0000; on VMware: 0x8300_0000.
    static SMP_STACK_BASE_PTR: u64;
}

/// Write CPU 0's actual MMU configuration to .bss.boot variables so
/// secondary CPUs can replicate the exact same setup.
///
/// Stores TTBR0, TTBR1, MAIR_EL1, and TCR_EL1 values.
/// On QEMU, these come from boot.S's setup_mmu. On Parallels, they come
/// from the UEFI loader's page_tables module (different TCR, different TTBRs).
/// Secondary CPUs in boot.S read these to configure MMU identically to CPU 0.
///
/// Includes cache clean + DSB so values are visible to secondary CPUs
/// which start with MMU off (uncached reads from physical memory).
///
/// Must be called before the first `release_cpu()` call.
pub fn set_smp_ttbrs() {
    // Helper: write a u64 to a .bss.boot variable via .rodata pointer indirection,
    // then clean the cache line so uncached readers (secondary CPUs) see it.
    unsafe fn write_bss_boot_var(ptr_ro: &u64, value: u64) {
        let var_phys = core::ptr::read_volatile(ptr_ro);
        let var_virt = (var_phys + 0xFFFF_0000_0000_0000u64) as *mut u64;
        core::ptr::write_volatile(var_virt, value);
        core::arch::asm!(
            "dc cvac, {addr}",
            addr = in(reg) var_virt,
            options(nostack),
        );
    }

    unsafe {
        let ttbr0: u64;
        let ttbr1: u64;
        let mair: u64;
        let tcr: u64;
        core::arch::asm!(
            "mrs {}, ttbr0_el1",
            "mrs {}, ttbr1_el1",
            "mrs {}, mair_el1",
            "mrs {}, tcr_el1",
            out(reg) ttbr0,
            out(reg) ttbr1,
            out(reg) mair,
            out(reg) tcr,
            options(nomem, nostack),
        );

        write_bss_boot_var(&SMP_TTBR0_PTR, ttbr0);
        write_bss_boot_var(&SMP_TTBR1_PTR, ttbr1);
        write_bss_boot_var(&SMP_MAIR_PTR, mair);
        write_bss_boot_var(&SMP_TCR_PTR, tcr);

        // Single DSB to ensure all cache cleans complete
        core::arch::asm!("dsb ish", options(nostack),);
    }
}

/// Set the UART physical address for secondary CPU boot debug output.
/// Must be called before `release_cpu()`.
///
/// Uses indirection through SMP_UART_PHYS_PTR because SMP_UART_PHYS lives in
/// .bss.boot (low physical memory) and direct ADRP from high-half Rust code
/// would overflow the +/-4GiB relocation range.
///
/// Includes cache clean + DSB so the value is visible to secondary CPUs
/// which start with MMU off (uncached reads from physical memory).
pub fn set_uart_phys(addr: u64) {
    unsafe {
        // SMP_UART_PHYS_PTR holds the physical address of SMP_UART_PHYS.
        // Add HHDM base to get the virtual address, then write through it.
        let phys = core::ptr::read_volatile(&SMP_UART_PHYS_PTR);
        let virt = phys + 0xFFFF_0000_0000_0000u64; // KERNEL_VIRT_BASE / HHDM
        let ptr = virt as *mut u64;
        core::ptr::write_volatile(ptr, addr);
        // Clean cache line to Point of Coherency so uncached reads see it
        core::arch::asm!(
            "dc cvac, {addr}",  // Clean by VA to PoC
            "dsb ish",          // Ensure completion
            addr = in(reg) ptr,
            options(nostack),
        );
    }
}

/// Set the physical base address of the per-CPU stack region.
/// Must be called before `release_cpu()`.
///
/// The stack base is `ram_base + 0x0300_0000` (48MB into RAM, after kernel image + BSS).
/// On QEMU/Parallels (ram at 0x40000000): 0x4300_0000.
/// On VMware (ram at 0x80000000): 0x8300_0000.
pub fn set_stack_base_phys(addr: u64) {
    unsafe {
        let phys = core::ptr::read_volatile(&SMP_STACK_BASE_PTR);
        let virt = phys + 0xFFFF_0000_0000_0000u64;
        let ptr = virt as *mut u64;
        core::ptr::write_volatile(ptr, addr);
        core::arch::asm!(
            "dc cvac, {addr}",
            "dsb ish",
            addr = in(reg) ptr,
            options(nostack),
        );
    }
}

/// Number of CPUs currently online (starts at 1 for the boot CPU).
static CPUS_ONLINE: AtomicU64 = AtomicU64::new(1);

/// Per-CPU online status flags.
static CPU_ONLINE: [AtomicBool; MAX_CPUS] = [
    AtomicBool::new(true), // CPU 0 is online at boot
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
];

const BRINGUP_STAGE_NOT_STARTED: u32 = 0;
const BRINGUP_STAGE_RUST_ENTRY: u32 = 1;
const BRINGUP_STAGE_PER_CPU_DATA_INITIALIZED: u32 = 2;
const BRINGUP_STAGE_KERNEL_PAGE_TABLE_RECORDED: u32 = 3;
const BRINGUP_STAGE_KERNEL_STACK_RECORDED: u32 = 4;
const BRINGUP_STAGE_ENTERING_GIC_CPU_INTERFACE_INIT: u32 = 5;
const BRINGUP_STAGE_GIC_CPU_INTERFACE_INITIALIZED: u32 = 6;
const BRINGUP_STAGE_ENTERING_TIMER_INIT: u32 = 7;
const BRINGUP_STAGE_TIMER_INITIALIZED: u32 = 8;
const BRINGUP_STAGE_ALLOCATING_IDLE_THREAD: u32 = 9;
const BRINGUP_STAGE_IDLE_THREAD_ALLOCATED: u32 = 10;
const BRINGUP_STAGE_REGISTERING_IDLE_THREAD: u32 = 11;
const BRINGUP_STAGE_IDLE_THREAD_REGISTERED: u32 = 12;
const BRINGUP_STAGE_INTERRUPTS_ENABLED: u32 = 13;
const BRINGUP_STAGE_ONLINE: u32 = 14;

/// One cache-line-isolated bring-up stage for a single CPU.
///
/// Secondary CPUs publish stages concurrently while CPU 0 samples them. If
/// adjacent stages share a cache line, those stores invalidate one another and
/// CPU 0's reads contend with every secondary on the same line, delaying the
/// bring-up this diagnostic is meant to observe.
#[repr(C, align(64))]
struct CpuBringupStage {
    value: AtomicU32,
    _padding: [u8; 60],
}

impl CpuBringupStage {
    const fn new() -> Self {
        Self {
            value: AtomicU32::new(BRINGUP_STAGE_NOT_STARTED),
            _padding: [0; 60],
        }
    }
}

const _: () = assert!(
    core::mem::size_of::<CpuBringupStage>() == 64,
    "CpuBringupStage must occupy exactly one cache line"
);
const _: () = assert!(
    core::mem::align_of::<CpuBringupStage>() == 64,
    "CpuBringupStage must be cache-line aligned"
);

static CPU_BRINGUP_STAGE: [CpuBringupStage; MAX_CPUS] =
    [const { CpuBringupStage::new() }; MAX_CPUS];

#[inline(always)]
fn set_bringup_stage(cpu_id: usize, stage: u32) {
    if let Some(cpu_stage) = CPU_BRINGUP_STAGE.get(cpu_id) {
        cpu_stage.value.store(stage, Ordering::Release);
    }
}

/// Return the most recently published bring-up stage for one CPU.
pub fn bringup_stage_of(cpu_id: usize) -> u32 {
    CPU_BRINGUP_STAGE
        .get(cpu_id)
        .map(|stage| stage.value.load(Ordering::Acquire))
        .unwrap_or(BRINGUP_STAGE_NOT_STARTED)
}

/// Return the static diagnostic name for a bring-up stage.
pub fn bringup_stage_name(stage: u32) -> &'static str {
    match stage {
        BRINGUP_STAGE_NOT_STARTED => "not-started",
        BRINGUP_STAGE_RUST_ENTRY => "rust-entry",
        BRINGUP_STAGE_PER_CPU_DATA_INITIALIZED => "per-cpu-data-initialized",
        BRINGUP_STAGE_KERNEL_PAGE_TABLE_RECORDED => "kernel-page-table-recorded",
        BRINGUP_STAGE_KERNEL_STACK_RECORDED => "kernel-stack-recorded",
        BRINGUP_STAGE_ENTERING_GIC_CPU_INTERFACE_INIT => "entering-gic-cpu-interface-init",
        BRINGUP_STAGE_GIC_CPU_INTERFACE_INITIALIZED => "gic-cpu-interface-initialized",
        BRINGUP_STAGE_ENTERING_TIMER_INIT => "entering-timer-init",
        BRINGUP_STAGE_TIMER_INITIALIZED => "timer-initialized",
        BRINGUP_STAGE_ALLOCATING_IDLE_THREAD => "allocating-idle-thread",
        BRINGUP_STAGE_IDLE_THREAD_ALLOCATED => "idle-thread-allocated",
        BRINGUP_STAGE_REGISTERING_IDLE_THREAD => "registering-idle-thread",
        BRINGUP_STAGE_IDLE_THREAD_REGISTERED => "idle-thread-registered",
        BRINGUP_STAGE_INTERRUPTS_ENABLED => "interrupts-enabled",
        BRINGUP_STAGE_ONLINE => "online",
        _ => "unknown",
    }
}

/// Sum the monotonic per-CPU bring-up stages into one progress signal.
pub fn bringup_progress() -> u64 {
    CPU_BRINGUP_STAGE
        .iter()
        .map(|stage| u64::from(stage.value.load(Ordering::Acquire)))
        .sum()
}

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

/// PSCI CPU_ON with 32-bit function ID via HVC.
fn psci_cpu_on_32(target_cpu: u64, entry_point: u64, context_id: u64) -> i64 {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "hvc #0",
            inout("x0") PSCI_CPU_ON_32 => ret,
            in("x1") target_cpu,
            in("x2") entry_point,
            in("x3") context_id,
            options(nomem, nostack),
        );
    }
    ret
}

/// PSCI CPU_ON with 64-bit function ID via SMC (EL3 firmware conduit).
///
/// This conduit is not attempted on VMware (EL1 guest, no EL3).
/// SMC would trap to EL2 and likely fault. HVC is the correct conduit.
fn psci_cpu_on_smc(target_cpu: u64, entry_point: u64, context_id: u64) -> i64 {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "smc #0",
            inout("x0") PSCI_CPU_ON_64 => ret,
            in("x1") target_cpu,
            in("x2") entry_point,
            in("x3") context_id,
            options(nomem, nostack),
        );
    }
    ret
}

fn psci_cpu_on_was_accepted(ret: i64) -> bool {
    matches!(
        ret,
        PSCI_RETURN_SUCCESS | PSCI_RETURN_ALREADY_ON | PSCI_RETURN_ON_PENDING
    )
}

fn psci_cpu_on_retry_backoff() {
    let reported_frequency_hz = crate::arch_impl::aarch64::timer::frequency_hz();
    let frequency_hz = if reported_frequency_hz == 0 {
        // P17/P19 in docs/polling-allowlist.md deliberately use a short 1MHz
        // boot fallback. P20/P21 instead fail closed because their deadlines
        // protect host harnesses.
        CNTVCT_FALLBACK_FREQUENCY_HZ
    } else {
        reported_frequency_hz
    };
    let backoff_ticks = (frequency_hz
        .saturating_mul(PSCI_CPU_ON_RETRY_BACKOFF_MICROSECONDS)
        / 1_000_000)
        .max(1);
    let start = crate::arch_impl::aarch64::timer::rdtsc();
    for _ in 0..PSCI_CPU_ON_BACKOFF_ITERATION_CAP {
        if crate::arch_impl::aarch64::timer::rdtsc().wrapping_sub(start) >= backoff_ticks {
            break;
        }
        core::hint::spin_loop();
    }
}

/// Release a secondary CPU using PSCI CPU_ON.
///
/// The CPU will start executing at `secondary_cpu_entry` in boot.S,
/// which sets up the stack and MMU, then calls `secondary_cpu_entry_rust(cpu_id)`.
///
/// Returns 0 for PSCI success, `ALREADY_ON`, or `ON_PENDING`; other negative
/// PSCI results are returned. The raw final code is preserved for diagnostics
/// through [`last_psci_return_code()`].
pub fn release_cpu(cpu_id: usize) -> i64 {
    if cpu_id == 0 || cpu_id >= MAX_CPUS {
        return -2; // INVALID_PARAMS
    }

    // Get the physical address of the secondary entry point in boot.S.
    // SECONDARY_CPU_ENTRY_PHYS holds the linker address (base 0x40080000).
    // On VMware, RAM starts at 0x80000000, so the actual physical address
    // is offset by ram_base_offset (0x40000000).
    let entry_phys = unsafe { core::ptr::read_volatile(&SECONDARY_CPU_ENTRY_PHYS) }
        + crate::platform_config::ram_base_offset();

    // MPIDR: Aff0 = cpu_id, all other affinity fields = 0
    // This is the standard layout for ARM virt machines (QEMU, Parallels, VMware)
    let target_mpidr = cpu_id as u64;

    // Context ID: pass cpu_id so the new CPU knows who it is
    let context_id = cpu_id as u64;

    let mut ret = PSCI_RETURN_NOT_ATTEMPTED;
    for attempt in 0..PSCI_CPU_ON_MAX_ATTEMPTS {
        // Preserve the established conduit order on every bounded attempt:
        // HVC64 first, then HVC32. SMC remains deliberately excluded.
        let hvc64_ret = psci_cpu_on(target_mpidr, entry_phys, context_id);
        ret = hvc64_ret;
        if !psci_cpu_on_was_accepted(ret) {
            if attempt == 0 {
                crate::serial_println!(
                    "[smp] CPU {}: HVC64 failed (ret={}), trying HVC32...",
                    cpu_id,
                    hvc64_ret
                );
            }
            ret = psci_cpu_on_32(target_mpidr, entry_phys, context_id);
        }
        LAST_PSCI_RETURN_CODE[cpu_id].store(ret, Ordering::Release);

        if psci_cpu_on_was_accepted(ret) {
            if attempt > 0 {
                crate::serial_println!(
                    "[smp] CPU {}: PSCI CPU_ON accepted after {} attempts (raw_status={})",
                    cpu_id,
                    attempt + 1,
                    ret
                );
            }
            return PSCI_RETURN_SUCCESS;
        }
        if ret != PSCI_RETURN_INTERNAL_FAILURE || attempt + 1 == PSCI_CPU_ON_MAX_ATTEMPTS {
            break;
        }
        psci_cpu_on_retry_backoff();
    }

    if ret != 0 {
        crate::serial_println!(
            "[smp] PSCI CPU_ON failed for CPU {}: ret={} (MPIDR={:#x} entry={:#x})",
            cpu_id,
            ret,
            target_mpidr,
            entry_phys
        );
    }

    ret
}

/// Last raw PSCI status observed for a CPU, retained for online-timeout diagnostics.
pub fn last_psci_return_code(cpu_id: usize) -> i64 {
    LAST_PSCI_RETURN_CODE
        .get(cpu_id)
        .map(|status| status.load(Ordering::Acquire))
        .unwrap_or(PSCI_RETURN_NOT_ATTEMPTED)
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
    let addr = crate::platform_config::uart_virt() as *mut u8;
    unsafe {
        core::ptr::write_volatile(addr, c);
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
    set_bringup_stage(cpu_id as usize, BRINGUP_STAGE_RUST_ENTRY);

    // Initialize per-CPU data (sets TPIDR_EL1 for this CPU)
    crate::per_cpu_aarch64::init_cpu(cpu_id as usize);
    set_bringup_stage(cpu_id as usize, BRINGUP_STAGE_PER_CPU_DATA_INITIALIZED);

    // Store the boot TTBR0 as the kernel page table for this CPU.
    let boot_ttbr0: u64;
    unsafe {
        core::arch::asm!("mrs {}, ttbr0_el1", out(reg) boot_ttbr0, options(nomem, nostack));
    }
    crate::per_cpu_aarch64::set_kernel_cr3(boot_ttbr0);
    set_bringup_stage(cpu_id as usize, BRINGUP_STAGE_KERNEL_PAGE_TABLE_RECORDED);

    // Set kernel stack top for this CPU.
    // boot.S sets SP to SMP_STACK_BASE_PHYS + (cpu_id+1)*0x200000 (physical),
    // then adds KERNEL_VIRT_BASE after enabling MMU.
    // This value is critical: when a user thread runs on this CPU and an
    // exception occurs, the kernel needs to switch to this stack.
    let kernel_stack_top = super::constants::percpu_kernel_stack_top(cpu_id as usize);
    crate::per_cpu_aarch64::set_kernel_stack_top(kernel_stack_top);
    set_bringup_stage(cpu_id as usize, BRINGUP_STAGE_KERNEL_STACK_RECORDED);

    // Initialize GIC CPU interface (GICC registers are banked per-CPU)
    set_bringup_stage(
        cpu_id as usize,
        BRINGUP_STAGE_ENTERING_GIC_CPU_INTERFACE_INIT,
    );
    super::gic::init_cpu_interface_secondary();
    set_bringup_stage(cpu_id as usize, BRINGUP_STAGE_GIC_CPU_INTERFACE_INITIALIZED);

    // Initialize timer for this CPU (arm virtual timer, enable PPI 27)
    set_bringup_stage(cpu_id as usize, BRINGUP_STAGE_ENTERING_TIMER_INIT);
    super::timer_interrupt::init_secondary();
    set_bringup_stage(cpu_id as usize, BRINGUP_STAGE_TIMER_INITIALIZED);

    // Create and register this CPU's idle thread with the scheduler.
    // This must happen before enabling interrupts — the scheduler needs
    // an idle thread for this CPU before timer interrupts fire.
    create_and_register_idle_thread(cpu_id as usize);

    // Enable interrupts so this CPU can handle timer ticks
    unsafe {
        super::cpu::enable_interrupts();
    }
    set_bringup_stage(cpu_id as usize, BRINGUP_STAGE_INTERRUPTS_ENABLED);

    // Mark this CPU as online (after all init is complete)
    if (cpu_id as usize) < MAX_CPUS {
        CPU_ONLINE[cpu_id as usize].store(true, Ordering::Release);
    }
    CPUS_ONLINE.fetch_add(1, Ordering::Release);
    set_bringup_stage(cpu_id as usize, BRINGUP_STAGE_ONLINE);

    // Idle loop — wait for interrupts, handle timer, participate in scheduling
    loop {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }
}

/// Create an idle thread for a secondary CPU and register it with the scheduler.
fn create_and_register_idle_thread(cpu_id: usize) {
    use crate::memory::arch_stub::VirtAddr;
    use crate::task::thread::{Thread, ThreadPrivilege, ThreadState};
    use alloc::boxed::Box;
    use alloc::format;

    set_bringup_stage(cpu_id, BRINGUP_STAGE_ALLOCATING_IDLE_THREAD);

    // Boot stack addresses — must match boot.S layout.
    let boot_stack_top = VirtAddr::new(super::constants::percpu_kernel_stack_top(cpu_id));
    let boot_stack_bottom = VirtAddr::new(super::constants::percpu_kernel_stack_bottom(cpu_id));
    let dummy_tls = VirtAddr::zero();

    let mut idle_task = Box::new(Thread::new(
        format!("swapper/{}", cpu_id),
        idle_thread_fn,
        boot_stack_top,
        boot_stack_bottom,
        dummy_tls,
        ThreadPrivilege::Kernel,
    ));
    set_bringup_stage(cpu_id, BRINGUP_STAGE_IDLE_THREAD_ALLOCATED);

    // CRITICAL: Set kernel_stack_top to this CPU's boot stack. Without this,
    // setup_idle_return_arm64 falls back to the per-CPU kernel_stack_top from
    // the last dispatched thread, causing the idle loop to run on that thread's
    // kernel stack and corrupt its SVC frame.
    idle_task.kernel_stack_top = Some(boot_stack_top);

    // Mark as running and already started (this CPU is already executing)
    idle_task.state = ThreadState::Running;
    idle_task.has_started = true;

    // Initialize context to idle_loop_arm64 at creation time to prevent
    // INSTRUCTION_ABORT at ELR=0x0 if dispatched before first timer save.
    idle_task.context.elr_el1 = super::context_switch::idle_loop_arm64 as *const () as u64;
    idle_task.context.spsr_el1 = 0x5; // EL1h, interrupts enabled

    // Set per-CPU current thread pointer
    let idle_task_ptr = &*idle_task as *const _ as *mut crate::task::thread::Thread;
    crate::per_cpu_aarch64::set_current_thread(idle_task_ptr);

    // Register with the global scheduler
    set_bringup_stage(cpu_id, BRINGUP_STAGE_REGISTERING_IDLE_THREAD);
    crate::task::scheduler::register_cpu_idle_thread(cpu_id, idle_task);
    set_bringup_stage(cpu_id, BRINGUP_STAGE_IDLE_THREAD_REGISTERED);
}

/// Idle thread function for secondary CPUs.
fn idle_thread_fn() {
    loop {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }
}
