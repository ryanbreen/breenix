//! Starting the application processors the MADT reports.
//!
//! The boot processor starts each enabled processor in MADT order with the
//! INIT-SIPI-SIPI sequence (Intel SDM Vol. 3A, 9.4.4.1), one at a time. The
//! startup IPI points the processor at a copy of `ap_trampoline.asm` on a page
//! below 1 MiB; the trampoline takes it to long mode on a small identity-mapped
//! page table and jumps to `ap_entry` on a stack allocated for it. `ap_entry`
//! switches to the master kernel page table, runs the per-CPU init every CPU
//! runs (`super::cpu_init::init_cpu`), registers the processor's idle thread
//! with the scheduler and marks the processor online. From then on the
//! processor takes its own LAPIC timer tick and dispatches like the boot CPU.
//!
//! A processor that has not come online within `ONLINE_TIMEOUT_MS` is sent
//! INIT again, which parks it in wait-for-SIPI, so it cannot later run the
//! trampoline with the next processor's stack. It is named in the boot log by
//! `super::smp::report_bring_up`.

use core::sync::atomic::{AtomicU64, Ordering};

use x86_64::registers::control::{Cr0, Cr4};
use x86_64::registers::model_specific::Efer;
use x86_64::VirtAddr;

use super::{apic, smp};
use crate::task::scheduler::MAX_CPUS;

extern "C" {
    static ap_trampoline_start: u8;
    static ap_trampoline_end: u8;
    static ap_trampoline_gdt: u8;
    static ap_trampoline_gdt_desc: u8;
    static ap_trampoline_pm32: u8;
    static ap_trampoline_pm32_ptr: u8;
    static ap_trampoline_lm64: u8;
    static ap_trampoline_lm64_ptr: u8;
    static ap_trampoline_pml4: u8;
    static ap_trampoline_efer: u8;
    static ap_trampoline_stack: u8;
    static ap_trampoline_cpu: u8;
    static ap_trampoline_entry: u8;
}

/// How long a started processor has to come online.
const ONLINE_TIMEOUT_MS: u64 = 500;

/// The boot processor's control registers, which every application processor
/// loads once it is on the master kernel page table.
static BSP_CR0: AtomicU64 = AtomicU64::new(0);
static BSP_CR4: AtomicU64 = AtomicU64::new(0);
static MASTER_CR3: AtomicU64 = AtomicU64::new(0);

/// Logical number + 1 of the processor that most recently reached `ap_entry`.
static ARRIVED: AtomicU64 = AtomicU64::new(0);

/// Top of the stack each application processor was started on, which becomes
/// its idle thread's kernel stack.
static STACK_TOP: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static STACK_BOTTOM: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

const PRESENT: u64 = 1 << 0;
const WRITABLE: u64 = 1 << 1;
const HUGE: u64 = 1 << 7;

/// Offset of a trampoline symbol from the trampoline's start.
fn offset(symbol: &u8) -> usize {
    // SAFETY: only the symbol's address is taken.
    let start = unsafe { &ap_trampoline_start } as *const u8 as usize;
    symbol as *const u8 as usize - start
}

fn spin_for_us(microseconds: u64) {
    let deadline = super::timer::rdtsc() + super::timer::frequency_hz() * microseconds / 1_000_000;
    while super::timer::rdtsc() < deadline {
        core::hint::spin_loop();
    }
}

/// Start every enabled processor the MADT reports, other than this one.
///
/// Runs on the boot processor during boot, after the scheduler and process
/// manager exist and before anything is placed on another CPU. Returns once
/// every started processor is online or has been given up on.
pub fn start_application_processors() {
    if !apic::active() || smp::enumeration_source() != smp::EnumerationSource::Madt {
        return;
    }
    let others = (0..smp::enumerated_cpu_count())
        .filter(|&index| {
            smp::apic_entry_enabled(index) && smp::apic_id_of(index) != Some(smp::bsp_apic_id())
        })
        .count();
    if others == 0 {
        return;
    }
    let Some(base) = crate::memory::frame_allocator::ap_trampoline_base() else {
        log::warn!("[smp] no usable page below 1 MiB for the AP trampoline; APs not started");
        return;
    };
    let Some(master) = crate::memory::kernel_page_table::master_kernel_pml4() else {
        log::warn!("[smp] no master kernel page table; APs not started");
        return;
    };

    BSP_CR0.store(Cr0::read_raw(), Ordering::Relaxed);
    BSP_CR4.store(Cr4::read_raw(), Ordering::Relaxed);
    MASTER_CR3.store(master.start_address().as_u64(), Ordering::Release);

    let phys = crate::memory::physical_memory_offset().as_u64();
    let page = |index: u64| (phys + base + index * 4096) as *mut u8;
    let pml4_phys = base + 0x1000;
    let pdpt_phys = base + 0x2000;
    let pd_phys = base + 0x3000;

    // SAFETY: the run is usable memory below the allocator's floor, reserved
    // for this trampoline (`ap_trampoline_base`), and mapped by the physical
    // memory window. The master PML4 is a live page table read here only.
    unsafe {
        let start = &ap_trampoline_start as *const u8;
        let len = &ap_trampoline_end as *const u8 as usize - start as usize;
        assert!(len <= 4096, "AP trampoline is larger than a page");
        core::ptr::copy_nonoverlapping(start, page(0), len);

        // The trampoline's page table: entry 0 identity-maps the first 2 MiB,
        // where the trampoline runs when paging comes on; every other entry is
        // the master table's, so the kernel image and the stacks are mapped.
        let master_entries = (phys + master.start_address().as_u64()) as *const u64;
        let pml4 = page(1) as *mut u64;
        let pdpt = page(2) as *mut u64;
        let pd = page(3) as *mut u64;
        for index in 0..512 {
            pml4.add(index)
                .write_volatile(master_entries.add(index).read_volatile());
            pdpt.add(index).write_volatile(0);
            pd.add(index).write_volatile(0);
        }
        pml4.write_volatile(pdpt_phys | PRESENT | WRITABLE);
        pdpt.write_volatile(pd_phys | PRESENT | WRITABLE);
        pd.write_volatile(PRESENT | WRITABLE | HUGE);

        let field = |symbol: &u8| page(0).add(offset(symbol));
        (field(&ap_trampoline_gdt_desc).add(2) as *mut u32)
            .write_unaligned((base as usize + offset(&ap_trampoline_gdt)) as u32);
        (field(&ap_trampoline_pm32_ptr) as *mut u32)
            .write_unaligned((base as usize + offset(&ap_trampoline_pm32)) as u32);
        (field(&ap_trampoline_lm64_ptr) as *mut u32)
            .write_unaligned((base as usize + offset(&ap_trampoline_lm64)) as u32);
        (field(&ap_trampoline_pml4) as *mut u32).write_unaligned(pml4_phys as u32);
        // LMA is the processor's report, not a control bit.
        let efer = (Efer::read_raw() & !(1 << 10)) | (1 << 8);
        (field(&ap_trampoline_efer) as *mut u32).write_unaligned(efer as u32);
        (field(&ap_trampoline_entry) as *mut u64).write_unaligned(ap_entry as usize as u64);
    }

    let startup_page = (base >> 12) as u8;
    for index in 0..smp::enumerated_cpu_count() {
        let Some(apic_id) = smp::apic_id_of(index) else {
            continue;
        };
        if !smp::apic_entry_enabled(index) || apic_id == smp::bsp_apic_id() {
            continue;
        }
        let cpu = smp::cpus_online() as usize;
        if cpu >= MAX_CPUS {
            log::warn!(
                "[smp] APIC id {} not started: this kernel addresses {} CPUs",
                apic_id,
                MAX_CPUS
            );
            continue;
        }
        if !start_one(cpu, apic_id, startup_page, base, phys) {
            smp::note_unanswered(index);
        }
    }

    // Device interrupts go to the last application processor online. A
    // softirq a device interrupt raises runs on the CPU that took it, at an
    // interrupt exit with nothing non-preemptible underneath, and the boot
    // processor runs the boot thread with preemption disabled until the boot
    // tests are under way: while user processes ran there would be none, so
    // network receive processing waited on the boot thread.
    let last = smp::cpus_online() as usize - 1;
    if last != 0 {
        if let Some(apic_id) = smp::cpu_apic_id(last) {
            super::ioapic::set_destination(apic_id);
            log::info!(
                "[smp] device interrupts routed to CPU {} (APIC id {})",
                last,
                apic_id
            );
        }
    }
}

/// Start the processor with local APIC id `apic_id` as logical CPU `cpu`.
/// Returns whether it came online.
fn start_one(cpu: usize, apic_id: u32, startup_page: u8, base: u64, phys: u64) -> bool {
    let stack = match crate::memory::kernel_stack::allocate_kernel_stack() {
        Ok(stack) => stack,
        Err(reason) => {
            log::warn!("[smp] no stack for CPU {}: {}", cpu, reason);
            return false;
        }
    };
    let stack_top = stack.top().as_u64();
    STACK_BOTTOM[cpu].store(stack.bottom().as_u64(), Ordering::Relaxed);
    STACK_TOP[cpu].store(stack_top, Ordering::Release);

    // SAFETY: the trampoline page was written by the caller and no processor
    // is running it: the previous one has arrived in `ap_entry`, or was sent
    // INIT after it did not.
    unsafe {
        let page0 = (phys + base) as *mut u8;
        (page0.add(offset(&ap_trampoline_stack)) as *mut u64).write_unaligned(stack_top);
        (page0.add(offset(&ap_trampoline_cpu)) as *mut u64).write_unaligned(cpu as u64);
    }
    core::sync::atomic::fence(Ordering::SeqCst);

    let send = |ipi| {
        if let Err(reason) = apic::send_ipi(apic_id, ipi) {
            log::warn!("[smp] IPI to APIC id {} failed: {}", apic_id, reason);
        }
    };
    send(apic::Ipi::Init);
    spin_for_us(10_000);
    send(apic::Ipi::Startup(startup_page));
    spin_for_us(200);
    if ARRIVED.load(Ordering::Acquire) != cpu as u64 + 1 {
        send(apic::Ipi::Startup(startup_page));
    }

    let deadline = super::timer::rdtsc() + super::timer::frequency_hz() * ONLINE_TIMEOUT_MS / 1000;
    while !smp::is_cpu_online(cpu) {
        if super::timer::rdtsc() >= deadline {
            send(apic::Ipi::Init);
            log::warn!(
                "[smp] APIC id {} did not come online as CPU {} within {} ms",
                apic_id,
                cpu,
                ONLINE_TIMEOUT_MS
            );
            // The stack is not reused: a processor that arrived late may
            // still have run on it before the INIT.
            core::mem::forget(stack);
            return false;
        }
        core::hint::spin_loop();
    }
    // The stack is the processor's idle-thread stack for the life of the
    // kernel, recorded in its idle thread.
    core::mem::forget(stack);
    true
}

/// Where an application processor arrives from the trampoline, in long mode
/// on the trampoline page table and on its own stack, with interrupts masked.
extern "C" fn ap_entry(cpu: u64) -> ! {
    // SAFETY: the master page table maps everything the trampoline table
    // does above its first entry, including this code and this stack, and the
    // boot processor's CR0/CR4 describe the same processor model.
    unsafe {
        core::arch::asm!(
            "mov cr3, {cr3}",
            "mov cr4, {cr4}",
            "mov cr0, {cr0}",
            cr3 = in(reg) MASTER_CR3.load(Ordering::Acquire),
            cr4 = in(reg) BSP_CR4.load(Ordering::Relaxed),
            cr0 = in(reg) BSP_CR0.load(Ordering::Relaxed),
            options(nostack, preserves_flags)
        );
    }
    ARRIVED.store(cpu + 1, Ordering::Release);
    let cpu = cpu as usize;

    super::cpu_init::init_cpu(cpu);
    register_idle_thread(cpu);
    smp::mark_online(cpu);

    log::info!("[smp] CPU {} online (APIC id {})", cpu, apic::id());

    // From here this processor is its idle thread. A dispatch away from it
    // later resumes it at `idle_loop`'s top on its own stack, abandoning this
    // frame.
    x86_64::instructions::interrupts::enable();
    crate::interrupts::context_switch::idle_loop()
}

/// Create this processor's idle thread on the stack it is running on, make it
/// the processor's current thread and register it with the scheduler.
fn register_idle_thread(cpu: usize) {
    use crate::task::thread::{Thread, ThreadPrivilege, ThreadState};
    use alloc::boxed::Box;

    let stack_top = VirtAddr::new(STACK_TOP[cpu].load(Ordering::Acquire));

    let mut idle = Box::new(Thread::new(
        alloc::format!("swapper/{}", cpu),
        idle_thread_entry,
        stack_top,
        VirtAddr::new(STACK_BOTTOM[cpu].load(Ordering::Relaxed)),
        VirtAddr::zero(),
        ThreadPrivilege::Kernel,
    ));
    idle.state = ThreadState::Running;
    idle.has_started = true;
    idle.kernel_stack_top = Some(stack_top);
    idle.context.rip = crate::interrupts::context_switch::idle_loop as *const () as u64;
    idle.context.rsp = stack_top.as_u64();
    idle.context.rflags = 0x202;

    let idle_ptr = &mut *idle as *mut Thread;
    crate::per_cpu::set_current_thread(idle_ptr);
    crate::per_cpu::set_idle_thread(idle_ptr);
    crate::per_cpu::set_kernel_stack_top(stack_top.as_u64());
    crate::per_cpu::update_tss_rsp0(stack_top.as_u64());
    crate::task::scheduler::register_cpu_idle_thread(cpu, idle);
}

/// The idle thread's nominal entry point. An application processor's idle
/// thread starts by running, never through a dispatch of this function.
fn idle_thread_entry() {
    crate::interrupts::context_switch::idle_loop()
}
