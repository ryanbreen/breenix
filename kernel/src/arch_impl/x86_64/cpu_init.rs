//! The per-CPU half of x86_64 bring-up: everything a CPU must do to its own
//! registers and tables before it can take interrupts, enter the scheduler and
//! run userspace.
//!
//! The boot processor runs `init_cpu(0)` once the machine-wide state it builds
//! on exists (memory, the IDT, the interrupt controller choice and the timer
//! calibration). Each application processor will run `init_cpu(n)` from its
//! entry path before it is marked online (#1179); nothing here may assume it
//! runs on the boot processor or runs only once per CPU.
//!
//! Before memory is up the boot processor also loads its GDT/TSS
//! (`gdt::init`), the IDT and its per-CPU data (`per_cpu::init`) so early
//! faults are reported; `init_cpu(0)` reloads them, which every step below
//! allows.

use core::arch::x86_64::__cpuid;

use super::{apic, smp};

/// Initialize the executing CPU as logical CPU `cpu`.
///
/// In order: this CPU's GDT and TSS (its own RSP0 and IST stacks), the shared
/// IDT, GS base and kernel GS base on its own `PerCpuData`, the syscall MSRs,
/// FPU/SSE/XSAVE state, its scheduler quantum, and the local APIC with its
/// scheduler tick. Runs with interrupts masked and restores the caller's
/// interrupt state.
pub fn init_cpu(cpu: usize) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        crate::gdt::load(cpu);
        crate::gdt::install_ist_stacks(cpu);
        crate::interrupts::load_idt();
        crate::per_cpu::load_cpu(cpu);
        crate::per_cpu::set_tss(crate::gdt::tss_ptr(cpu));

        crate::syscall::init();

        let xsave = init_fpu();

        // A full quantum, charged from now: the first tick on this CPU must
        // not bill it for the time before its timer started.
        crate::per_cpu::take_quantum_ticks(crate::time::get_ticks());
        crate::interrupts::timer::reset_quantum();

        let apic_id = if apic::active() {
            apic::init_local();
            let id = apic::id();
            smp::set_cpu_apic_id(cpu, id);
            apic::start_local_timer();
            id
        } else {
            // PIC delivery: the PIT tick is machine-wide and arrives at the
            // BSP, the only CPU that runs without a local APIC.
            let id = smp::bsp_apic_id();
            smp::set_cpu_apic_id(cpu, id);
            id
        };

        log::info!(
            "[cpu-init] cpu={} apic_id={} gs_base={:#x} tss={:#x} xsave={} tick={}",
            cpu,
            apic_id,
            crate::per_cpu::cpu_data(cpu) as u64,
            crate::gdt::tss_ptr(cpu) as u64,
            xsave,
            if apic::active() { "lapic" } else { "pit" }
        );
    });
}

/// Enable x87, SSE and, where the CPU has it, XSAVE for user code on the
/// executing CPU. Returns whether XSAVE was enabled.
///
/// An application processor leaves its startup sequence with CR0.EM set and
/// CR4.OSFXSR clear, where every SSE instruction raises #UD; the firmware
/// configures the boot processor the same way this does. The kernel itself is
/// built soft-float and executes none of these instructions.
fn init_fpu() -> bool {
    use x86_64::registers::control::{Cr0, Cr0Flags, Cr4, Cr4Flags};
    use x86_64::registers::xcontrol::{XCr0, XCr0Flags};

    // SAFETY: CPUID leaf 1 is architecturally present on x86_64.
    let leaf1 = unsafe { __cpuid(1) };
    let xsave = leaf1.ecx & (1 << 26) != 0;
    let avx = leaf1.ecx & (1 << 28) != 0;

    unsafe {
        Cr0::update(|cr0| {
            cr0.remove(Cr0Flags::EMULATE_COPROCESSOR | Cr0Flags::TASK_SWITCHED);
            cr0.insert(Cr0Flags::MONITOR_COPROCESSOR | Cr0Flags::NUMERIC_ERROR);
        });
        Cr4::update(|cr4| {
            cr4.insert(Cr4Flags::OSFXSR | Cr4Flags::OSXMMEXCPT_ENABLE);
            if xsave {
                cr4.insert(Cr4Flags::OSXSAVE);
            }
        });
        if xsave {
            let mut features = XCr0Flags::X87 | XCr0Flags::SSE;
            if avx {
                features |= XCr0Flags::AVX;
            }
            XCr0::write(features);
        }
        core::arch::asm!("fninit", options(nomem, nostack));
    }
    xsave
}
