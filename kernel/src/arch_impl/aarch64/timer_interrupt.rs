//! ARM64 Timer Interrupt Handler
//!
//! This module provides the timer interrupt handler for ARM64, integrating
//! with the scheduler for preemptive multitasking.
//!
//! The ARM64 Generic Timer (CNTP_EL1 or CNTV_EL0) provides periodic interrupts.
//! Unlike x86_64 which uses the PIC/APIC, ARM64 uses the GIC (Generic Interrupt
//! Controller) to route timer interrupts.
//!
//! Timer Interrupt Flow:
//! 1. Timer fires (IRQ 27 = virtual timer PPI)
//! 2. GIC routes interrupt to handle_irq()
//! 3. handle_irq() calls timer_interrupt_handler()
//! 4. Handler updates time, checks quantum, sets need_resched
//! 5. On exception return, check need_resched and perform context switch if needed

use crate::task::scheduler;
use crate::tracing::providers::irq::trace_timer_tick;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Virtual timer interrupt ID (PPI 27) — used on QEMU/Parallels
pub const VIRT_TIMER_IRQ: u32 = 27;

/// Physical timer interrupt ID (PPI 30) — used on VMware Fusion
pub const PHYS_TIMER_IRQ: u32 = 30;

/// Get the active timer IRQ based on platform
pub fn timer_irq() -> u32 {
    if crate::platform_config::use_physical_timer() {
        PHYS_TIMER_IRQ
    } else {
        VIRT_TIMER_IRQ
    }
}

/// Time quantum in timer ticks (10 ticks = ~50ms at 200Hz)
const TIME_QUANTUM: u32 = 10;

/// Default timer ticks per interrupt (fallback for 24MHz clock)
/// This value is overwritten at init() with the dynamically calculated value
const DEFAULT_TICKS_PER_INTERRUPT: u64 = 24_000; // For 24MHz clock = ~1ms

/// Target timer frequency in Hz (1000 Hz = 1ms per interrupt)
const TARGET_TIMER_HZ: u64 = 1000;

/// Dynamically calculated ticks per interrupt based on actual timer frequency
/// Set during init() and used by the interrupt handler for consistent timing
pub static TICKS_PER_INTERRUPT: AtomicU64 = AtomicU64::new(DEFAULT_TICKS_PER_INTERRUPT);

/// Per-CPU time quantum counters.
/// Each CPU decrements its own quantum independently.
static CURRENT_QUANTUM: [AtomicU32; crate::arch_impl::aarch64::constants::MAX_CPUS] = [
    AtomicU32::new(TIME_QUANTUM),
    AtomicU32::new(TIME_QUANTUM),
    AtomicU32::new(TIME_QUANTUM),
    AtomicU32::new(TIME_QUANTUM),
    AtomicU32::new(TIME_QUANTUM),
    AtomicU32::new(TIME_QUANTUM),
    AtomicU32::new(TIME_QUANTUM),
    AtomicU32::new(TIME_QUANTUM),
];

/// Per-CPU SPSR_EL1 snapshot, updated every timer tick.
/// Index = CPU ID, value = SPSR_EL1 (saved PSTATE from before the interrupt).
/// Bits [9:6] contain the DAIF bits that were active when the CPU was interrupted.
pub static TIMER_TICK_DAIF: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

/// Per-CPU tick counter, incremented every timer tick.
/// If CPU 0 stops receiving interrupts, its count will stall while others advance.
pub static TIMER_TICK_COUNT: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

/// Per-CPU idle loop iteration counter, incremented before each WFI.
/// If CPU 0 stops receiving timer interrupts but this counter advances,
/// it means CPU 0 IS in the idle loop and WFI returns (probably due to
/// non-timer interrupts) but the timer interrupt is not being delivered.
pub static IDLE_LOOP_COUNT: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

/// Per-CPU: hardware MPIDR Aff0 at last timer tick (index = software cpu_id).
/// Used to detect cpu_id() returning wrong values after per-CPU data corruption.
/// 0xDEAD = never updated.
pub static TIMER_TICK_HW_CPU: [AtomicU64; 8] = [const { AtomicU64::new(0xDEAD) }; 8];

/// Per-CPU: tick count indexed by HARDWARE cpu (MPIDR & 0xFF).
/// Compared against TIMER_TICK_COUNT (indexed by software cpu_id) to find misattribution.
pub static TIMER_TICK_HW_COUNT: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

/// Per-CPU CNTV_CTL_EL0 snapshot at last timer tick.
/// Index = CPU ID, value = CNTV_CTL_EL0 register.
/// Bit 0: ENABLE, Bit 1: IMASK, Bit 2: ISTATUS.
pub static TIMER_TICK_CNTV_CTL: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

/// Whether the timer is initialized
static TIMER_INITIALIZED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Check if the timer interrupt has been initialized (for AHCI wfi/wfe decision).
pub fn timer_is_running() -> bool {
    TIMER_INITIALIZED.load(core::sync::atomic::Ordering::Relaxed)
}

/// Total timer interrupt count (for frequency verification)
static TIMER_INTERRUPT_COUNT: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "boot_tests")]
static RESET_QUANTUM_CALL_COUNT: AtomicU64 = AtomicU64::new(0);

// ─── Soft Lockup Detector ────────────────────────────────────────────────────
//
// Detects when no context switch has occurred for LOCKUP_THRESHOLD_TICKS timer
// interrupts (~5 seconds at 1000 Hz). When triggered, dumps diagnostic state to
// serial using lock-free raw_serial_str(). Fires once per stall, resets when
// context switches resume.

/// Threshold in timer ticks before declaring a soft lockup (5 seconds at 200 Hz)
const LOCKUP_THRESHOLD_TICKS: u64 = 200 * 5;

/// Last observed context switch count (CPU 0 only)
static WATCHDOG_LAST_CTX_SWITCH: AtomicU64 = AtomicU64::new(0);

/// Last observed syscall count (CPU 0 only, tracks system liveness)
static WATCHDOG_LAST_SYSCALL: AtomicU64 = AtomicU64::new(0);

/// Timer tick when progress was last observed (ctx switch OR syscall)
static WATCHDOG_LAST_PROGRESS_TICK: AtomicU64 = AtomicU64::new(0);

/// Whether we've already reported a lockup (avoid spamming serial)
static WATCHDOG_REPORTED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Initialize the timer interrupt system
///
/// Sets up the virtual timer to fire periodically for scheduling.
pub fn init() {
    if TIMER_INITIALIZED.load(Ordering::Relaxed) {
        return;
    }

    // Get the timer frequency from hardware
    let freq = super::timer::frequency_hz();
    log::info!("ARM64 timer interrupt init: frequency = {} Hz", freq);

    // Calculate ticks per interrupt for target Hz scheduling rate
    // For 62.5 MHz clock: 62_500_000 / 200 = 312_500 ticks
    // For 24 MHz clock: 24_000_000 / 200 = 120_000 ticks
    let ticks_per_interrupt = if freq > 0 {
        freq / TARGET_TIMER_HZ
    } else {
        DEFAULT_TICKS_PER_INTERRUPT
    };

    // Store the calculated value for use in the interrupt handler
    TICKS_PER_INTERRUPT.store(ticks_per_interrupt, Ordering::Release);

    crate::serial_println!(
        "[timer] Timer configured for ~{} Hz ({} ticks per interrupt)",
        TARGET_TIMER_HZ,
        ticks_per_interrupt
    );

    // Enable the timer interrupt in the GIC
    use crate::arch_impl::aarch64::gic;
    use crate::arch_impl::traits::InterruptController;

    if crate::platform_config::is_vmware() {
        // VMware: enable BOTH virtual (PPI 27) and physical (PPI 30) timer PPIs
        // so we can discover which one actually fires
        gic::Gicv2::enable_irq(VIRT_TIMER_IRQ as u8);
        gic::Gicv2::enable_irq(PHYS_TIMER_IRQ as u8);

        // Arm BOTH timers using CVAL (absolute compare value)
        unsafe {
            // Virtual timer: read counter, set future CVAL, enable
            let vcnt: u64;
            core::arch::asm!("mrs {}, cntvct_el0", out(reg) vcnt, options(nomem, nostack));
            let vcval = vcnt.wrapping_add(ticks_per_interrupt);
            core::arch::asm!("msr cntv_cval_el0, {}", in(reg) vcval, options(nomem, nostack));
            core::arch::asm!("msr cntv_ctl_el0, {}", in(reg) 1u64, options(nomem, nostack));
            // Physical timer: read counter, set future CVAL, enable
            let pcnt: u64;
            core::arch::asm!("mrs {}, cntpct_el0", out(reg) pcnt, options(nomem, nostack));
            let pcval = pcnt.wrapping_add(ticks_per_interrupt);
            core::arch::asm!("msr cntp_cval_el0, {}", in(reg) pcval, options(nomem, nostack));
            core::arch::asm!("msr cntp_ctl_el0, {}", in(reg) 1u64, options(nomem, nostack));
        }
        crate::serial_println!(
            "[timer] VMware: armed BOTH virtual (PPI 27) and physical (PPI 30) timers"
        );

        // Read back timer state to verify writes took effect
        let (vctl, vtval, pctl, ptval): (u64, u64, u64, u64);
        unsafe {
            core::arch::asm!("mrs {}, cntv_ctl_el0", out(reg) vctl, options(nomem, nostack));
            core::arch::asm!("mrs {}, cntv_tval_el0", out(reg) vtval, options(nomem, nostack));
            core::arch::asm!("mrs {}, cntp_ctl_el0", out(reg) pctl, options(nomem, nostack));
            core::arch::asm!("mrs {}, cntp_tval_el0", out(reg) ptval, options(nomem, nostack));
        }
        crate::serial_println!(
            "[timer] CNTV_CTL={:#x} CNTV_TVAL={} CNTP_CTL={:#x} CNTP_TVAL={}",
            vctl,
            vtval as i64,
            pctl,
            ptval as i64
        );

        // Dump GIC state for diagnosis
        dump_gic_state();

        // Read DAIF to verify IRQs/FIQs are unmaskable
        let daif: u64;
        unsafe {
            core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack));
        }
        crate::serial_println!(
            "[timer] DAIF={:#x} (IRQ={} FIQ={})",
            daif,
            if daif & (1 << 7) != 0 {
                "MASKED"
            } else {
                "unmasked"
            },
            if daif & (1 << 6) != 0 {
                "MASKED"
            } else {
                "unmasked"
            }
        );

        // Read Group enable state (ICC_IGRPEN0_EL1 only accessible when DS=1)
        let grpen1: u64;
        unsafe {
            core::arch::asm!("mrs {}, icc_igrpen1_el1", out(reg) grpen1, options(nomem, nostack));
        }
        if gic::ds_enabled() {
            let grpen0: u64;
            unsafe {
                core::arch::asm!("mrs {}, icc_igrpen0_el1", out(reg) grpen0, options(nomem, nostack));
            }
            crate::serial_println!(
                "[timer] ICC_IGRPEN0={:#x} ICC_IGRPEN1={:#x} USE_GROUP0={} DS=1",
                grpen0,
                grpen1,
                crate::arch_impl::aarch64::gic::use_group0()
            );
        } else {
            crate::serial_println!(
                "[timer] ICC_IGRPEN1={:#x} USE_GROUP0={} DS=0 (IGRPEN0 inaccessible)",
                grpen1,
                crate::arch_impl::aarch64::gic::use_group0()
            );
        }
    } else {
        // Non-VMware: use the selected timer only
        arm_timer(ticks_per_interrupt);
        let irq = timer_irq();
        gic::Gicv2::enable_irq(irq as u8);
        crate::serial_println!(
            "[timer] Using {} timer (PPI {})",
            if irq == PHYS_TIMER_IRQ {
                "physical"
            } else {
                "virtual"
            },
            irq
        );
    }

    TIMER_INITIALIZED.store(true, Ordering::Release);
    log::info!("ARM64 timer interrupt initialized");
}

/// Arm the timer to fire after `ticks` counter increments from now.
///
/// Uses CVAL (absolute compare value) instead of TVAL, and never disables the
/// timer (ENABLE stays 1 at all times). The re-arm sequence is:
///   1. Read current counter (CNTVCT or CNTPCT)
///   2. Write CVAL = counter + ticks (future deadline)
///   3. Write CTL = 1 (ENABLE=1, IMASK=0) — unmasks the timer
///
/// This matches Linux's `set_next_event` pattern. The caller (timer interrupt
/// handler) sets IMASK=1 at handler entry to acknowledge the interrupt to the
/// hypervisor's vtimer masking logic, then this function clears IMASK when the
/// new deadline is safely in the future (ISTATUS=0).
fn arm_timer(ticks: u64) {
    if crate::platform_config::use_physical_timer() {
        unsafe {
            // Read current physical counter
            let cnt: u64;
            core::arch::asm!("mrs {}, cntpct_el0", out(reg) cnt, options(nomem, nostack));
            // Write absolute compare value
            let cval = cnt.wrapping_add(ticks);
            core::arch::asm!("msr cntp_cval_el0, {}", in(reg) cval, options(nomem, nostack));
            // Enable and unmask: ENABLE=1, IMASK=0 → CTL=1
            core::arch::asm!("msr cntp_ctl_el0, {}", in(reg) 1u64, options(nomem, nostack));
            core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
        }
    } else {
        unsafe {
            // Read current virtual counter
            let cnt: u64;
            core::arch::asm!("mrs {}, cntvct_el0", out(reg) cnt, options(nomem, nostack));
            // Write absolute compare value (matches Linux's set_next_event)
            let cval = cnt.wrapping_add(ticks);
            core::arch::asm!("msr cntv_cval_el0, {}", in(reg) cval, options(nomem, nostack));
            // Enable and unmask: ENABLE=1, IMASK=0 → CTL=1
            core::arch::asm!("msr cntv_ctl_el0, {}", in(reg) 1u64, options(nomem, nostack));
            core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
        }
    }
}

/// Timer interrupt handler - minimal work in interrupt context
///
/// This is called from handle_irq() when IRQ 27 (virtual timer) fires.
/// Each CPU fires its own timer (PPI 27 is per-CPU). The handler:
/// 1. Re-arms the timer for the next interrupt
/// 2. CPU 0 only: updates global wall clock time
/// 3. CPU 0 only: polls keyboard input
/// 4. All CPUs: decrements per-CPU time quantum
/// 5. CPU 0 only: sets need_resched if quantum expired (Phase 2: only CPU 0 schedules)
#[no_mangle]
pub extern "C" fn timer_interrupt_handler() {
    // Enter IRQ context (increment HARDIRQ count)
    crate::per_cpu_aarch64::irq_enter();

    let cpu_id = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;

    // Snapshot SPSR_EL1 for this CPU — this is the saved PSTATE from BEFORE
    // the interrupt was taken.  Bits [9:6] are the pre-interrupt DAIF mask.
    // Reading DAIF inside the handler always yields 0x3c0 (all masked on entry),
    // which is useless.  SPSR_EL1 tells us the actual pre-interrupt state.
    let spsr: u64;
    unsafe {
        core::arch::asm!("mrs {}, spsr_el1", out(reg) spsr, options(nomem, nostack));
    }
    if cpu_id < 8 {
        TIMER_TICK_DAIF[cpu_id].store(spsr, Ordering::Relaxed);
        TIMER_TICK_COUNT[cpu_id].fetch_add(1, Ordering::Relaxed);
    }

    // Mask the timer interrupt at the source (set IMASK=1, keep ENABLE=1).
    // This de-asserts the PPI line, which signals the hypervisor (Parallels/HVF)
    // that the guest has acknowledged the interrupt. Without this, the HVF
    // vtimer stays permanently masked because it never sees the guest acknowledge.
    // Linux does the same: arch_timer_handler_virt sets IMASK before calling
    // the clockevents handler.
    //
    // For VMware: write to BOTH virtual and physical timer CTLs (matching the
    // existing dual-write pattern since we don't know which timer fires).
    // For QEMU/Parallels: write to virtual timer only.
    if crate::platform_config::is_vmware() {
        unsafe {
            // CTL = ENABLE(1) | IMASK(1<<1) = 0b11 = 3
            core::arch::asm!("msr cntv_ctl_el0, {}", in(reg) 3u64, options(nomem, nostack));
            core::arch::asm!("msr cntp_ctl_el0, {}", in(reg) 3u64, options(nomem, nostack));
            core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
        }
    } else {
        unsafe {
            // CTL = ENABLE(1) | IMASK(1<<1) = 0b11 = 3
            core::arch::asm!("msr cntv_ctl_el0, {}", in(reg) 3u64, options(nomem, nostack));
            core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
        }
    }

    // Snapshot CNTV_CTL_EL0 for this CPU — captures the timer hardware state
    // after we set IMASK.  This should show CTL=3 (ENABLE=1, IMASK=1).
    let cntv_ctl: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntv_ctl_el0", out(reg) cntv_ctl, options(nomem, nostack));
    }
    if cpu_id < 8 {
        TIMER_TICK_CNTV_CTL[cpu_id].store(cntv_ctl, Ordering::Relaxed);
    }

    // Track hardware CPU identity vs software cpu_id for mismatch detection.
    let hw_cpu: u64;
    unsafe {
        core::arch::asm!("mrs {}, mpidr_el1", out(reg) hw_cpu, options(nomem, nostack));
    }
    let hw_cpu_id = (hw_cpu & 0xFF) as usize;
    if cpu_id < 8 {
        TIMER_TICK_HW_CPU[cpu_id].store(hw_cpu_id as u64, Ordering::Relaxed);
    }
    if hw_cpu_id < 8 {
        TIMER_TICK_HW_COUNT[hw_cpu_id].fetch_add(1, Ordering::Relaxed);
    }

    // Load ticks value early (before handler work) but defer the actual re-arm
    // until the very end. IMASK stays set (=1) throughout the handler, giving
    // the Parallels/HVF VMM time to observe the acknowledge via VM exits that
    // occur during the handler work (atomic ops, MMIO reads, etc.).
    let ticks = TICKS_PER_INTERRUPT.load(Ordering::Relaxed);

    // CPU 0 only: update global wall clock time (single atomic operation)
    if cpu_id == 0 {
        crate::time::timer_interrupt();
    }

    // Trace timer tick (lock-free counter + optional event recording)
    trace_timer_tick(crate::time::get_ticks());

    // Track idle ticks per CPU (for per-CPU utilization in /proc/stat)
    if scheduler::is_cpu_idle(cpu_id) {
        crate::tracing::providers::counters::IDLE_TICK_TOTAL.increment();
    }

    // Increment timer interrupt counter (used for debugging when needed)
    let _count = TIMER_INTERRUPT_COUNT.fetch_add(1, Ordering::Relaxed) + 1;

    // CPU 0 only: poll input devices (single-device, not safe from multiple CPUs)
    if cpu_id == 0 {
        poll_keyboard_to_stdin();
        crate::drivers::usb::ehci::poll_keyboard();
        crate::drivers::usb::xhci::poll_hid_events();
        if (crate::drivers::virtio::net_pci::is_initialized()
            || crate::drivers::e1000::is_initialized())
            && _count % 10 == 0
        {
            crate::task::softirqd::raise_softirq(crate::task::softirqd::SoftirqType::NetRx);
        }
    }

    // CPU 0 only: soft lockup detector
    if cpu_id == 0 {
        check_soft_lockup(_count);
    }

    // CPU 0 only: ready-queue safety net.
    //
    // Every 1000 ticks (~1 second) scan for threads that are in state=Ready
    // but are not in the ready queue, not current on any CPU, and not in a
    // deferred-requeue slot.  Any such thread is "stuck" — it will never run
    // without intervention.  We rescue it by adding it back to the ready queue
    // and sending a reschedule IPI to idle CPUs.
    //
    // This is a SAFETY NET, not a fix for the root cause.  When it fires it
    // prints "[SCHED_RESCUE] stuck tid=N" to the serial port so the exact
    // thread ID and its blocked_in_syscall flag are visible for diagnosis.
    //
    // Uses try_lock to avoid blocking the timer handler; if the scheduler
    // lock is contended we simply skip this tick and retry next second.
    if cpu_id == 0 && _count % 1000 == 0 {
        crate::task::scheduler::rescue_stuck_ready_threads_try();
    }

    // CPU 0 only: record heartbeat as trace event (lock-free, ~5 instructions)
    // All xHCI diagnostic atomics remain for GDB inspection; no serial output.
    if cpu_id == 0 {
        static CPU0_TICK: AtomicU64 = AtomicU64::new(0);
        let cpu0_tick = CPU0_TICK.fetch_add(1, Ordering::Relaxed) + 1;
        if cpu0_tick % 2000 == 0 {
            crate::tracing::providers::irq::trace_heartbeat_marker(
                (cpu0_tick / TARGET_TIMER_HZ) as u32,
            );
        }
    }

    // Cross-CPU timer rescue: if CPU 0's timer appears dead (tick count frozen
    // while other CPUs are advancing), send an SGI_TIMER_REARM IPI to CPU 0.
    // This forces CPU 0 to re-arm its vtimer from inside an interrupt handler
    // context, which the Parallels/HVF VMM will observe on the SGI's VM exit.
    // Only CPU 1 does this check (every 100 ticks) to avoid IPI storms.
    if cpu_id == 1 && _count % 100 == 0 {
        let cpu0_ticks = TIMER_TICK_COUNT[0].load(Ordering::Relaxed);
        let my_ticks = TIMER_TICK_COUNT[1].load(Ordering::Relaxed);
        // If CPU 0 has ≤10 ticks and we have >100, CPU 0's timer is dead
        if cpu0_ticks <= 10 && my_ticks > 100 {
            crate::arch_impl::aarch64::gic::send_sgi(
                crate::arch_impl::aarch64::constants::SGI_TIMER_REARM as u8,
                0, // target CPU 0
            );
        }
    }

    // Decrement per-CPU quantum and check for reschedule
    let quantum_idx = if cpu_id < crate::arch_impl::aarch64::constants::MAX_CPUS {
        cpu_id
    } else {
        0
    };
    let old_quantum = CURRENT_QUANTUM[quantum_idx].fetch_sub(1, Ordering::Relaxed);
    if old_quantum <= 1 {
        // Quantum expired - request reschedule (all CPUs participate)
        scheduler::set_need_resched();
        CURRENT_QUANTUM[quantum_idx].store(TIME_QUANTUM, Ordering::Relaxed);
    }

    // IDLE CPU FAST PATH: If this CPU is running its idle thread, always
    // request reschedule on every timer tick. This ensures that threads
    // added to the ready queue (by unblock() on another CPU) are picked up
    // within one timer tick (~5ms) instead of waiting for a full quantum
    // (~50ms). The scheduling decision quickly returns None if the ready
    // queue is empty, so the overhead is negligible for idle CPUs.
    if scheduler::is_cpu_idle(cpu_id) {
        scheduler::set_need_resched();
    }

    // Re-arm the timer at the very end of the handler.
    // IMASK has been 1 (set at handler entry) throughout all the handler work
    // above, giving the Parallels/HVF VMM multiple VM exit opportunities to
    // observe the IMASK=1 acknowledge. Now we write a future CVAL and clear
    // IMASK (CTL=1), matching Linux's set_next_event pattern.
    if crate::platform_config::is_vmware() {
        // VMware: re-arm both timers using CVAL (absolute compare value)
        unsafe {
            let vcnt: u64;
            core::arch::asm!("mrs {}, cntvct_el0", out(reg) vcnt, options(nomem, nostack));
            let vcval = vcnt.wrapping_add(ticks);
            core::arch::asm!("msr cntv_cval_el0, {}", in(reg) vcval, options(nomem, nostack));
            core::arch::asm!("msr cntv_ctl_el0, {}", in(reg) 1u64, options(nomem, nostack));
            let pcnt: u64;
            core::arch::asm!("mrs {}, cntpct_el0", out(reg) pcnt, options(nomem, nostack));
            let pcval = pcnt.wrapping_add(ticks);
            core::arch::asm!("msr cntp_cval_el0, {}", in(reg) pcval, options(nomem, nostack));
            core::arch::asm!("msr cntp_ctl_el0, {}", in(reg) 1u64, options(nomem, nostack));
            core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
        }
    } else {
        arm_timer(ticks);
    }

    // Exit IRQ context (decrement HARDIRQ count)
    crate::per_cpu_aarch64::irq_exit();
}

/// Dump GIC register state for VMware timer debugging.
/// Probes multiple known GIC addresses to discover working hardware.
fn dump_gic_state() {
    let gic_ver = crate::platform_config::gic_version();
    let gicd_base = crate::platform_config::gicd_base_phys();
    let gicc_base = crate::platform_config::gicc_base_phys();
    let gicr_base = crate::platform_config::gicr_base_phys();
    crate::serial_println!(
        "[timer] GIC version={} GICD={:#x} GICC={:#x} GICR={:#x}",
        gic_ver,
        gicd_base,
        gicc_base,
        gicr_base
    );

    const HHDM: u64 = 0xFFFF_0000_0000_0000;

    // Probe multiple known GIC addresses to find which one responds
    // 0xFFFFFFFF means non-functional (unmapped or no hardware)
    let probe_addrs: &[(u64, &str)] = &[
        (gicd_base, "reported GICD"),
        (gicc_base, "reported GICC"),
        (gicr_base, "reported GICR"),
        (0x0800_0000, "QEMU GICv2 GICD"),
        (0x0801_0000, "QEMU GICv2 GICC"),
        (0x0201_0000, "Parallels GICD"),
        (0x0250_0000, "Parallels GICR"),
        (0x3FFF_0000, "near UART region"),
    ];

    for &(addr, label) in probe_addrs {
        if addr == 0 {
            continue;
        }
        // Use the fault-tolerant probe so a non-responding address (DFSC=0x10)
        // doesn't crash the kernel.  This is especially important on M5 Max
        // where the loader may report a GICR base that causes an External Abort.
        let ctlr = crate::arch_impl::aarch64::gic::probe_mmio_u32(addr as usize);
        let pidr2 = crate::arch_impl::aarch64::gic::probe_mmio_u32(addr as usize + 0xFE8);
        match (ctlr, pidr2) {
            (None, _) | (_, None) => {
                crate::serial_println!(
                    "[gic-probe] {:#010x} ({}) FAULT (External Abort)",
                    addr,
                    label
                );
            }
            (Some(c), Some(p)) => {
                let ok = c != 0xFFFF_FFFF;
                crate::serial_println!(
                    "[gic-probe] {:#010x} ({}) CTLR={:#010x} PIDR2={:#010x} {}",
                    addr,
                    label,
                    c,
                    p,
                    if ok { "<<< FOUND" } else { "" }
                );
            }
        }
    }

    // Also read GICv3 system registers (these always work)
    let (pmr, grpen, bpr, sre): (u64, u64, u64, u64);
    unsafe {
        core::arch::asm!("mrs {}, icc_pmr_el1", out(reg) pmr, options(nomem, nostack));
        core::arch::asm!("mrs {}, icc_igrpen1_el1", out(reg) grpen, options(nomem, nostack));
        core::arch::asm!("mrs {}, icc_bpr1_el1", out(reg) bpr, options(nomem, nostack));
        core::arch::asm!("mrs {}, icc_sre_el1", out(reg) sre, options(nomem, nostack));
    }
    crate::serial_println!(
        "[timer] ICC regs: PMR={:#x} GRPEN1={:#x} BPR1={:#x} SRE={:#x}",
        pmr,
        grpen,
        bpr,
        sre
    );

    // Read GICD registers
    unsafe {
        let gicd = (HHDM + gicd_base) as *const u32;
        let gicd_ctlr = core::ptr::read_volatile(gicd.add(0));
        let gicd_isenabler0 = core::ptr::read_volatile(gicd.byte_add(0x100));
        crate::serial_println!(
            "[timer] GICD@{:#x}: CTLR={:#x} ISENABLER0={:#x}",
            gicd_base,
            gicd_ctlr,
            gicd_isenabler0
        );
    }

    // Read GICR registers (redistributor manages PPIs on GICv3).
    // Only read if the GICR base has been validated by the GIC probe mechanism.
    // On M5 Max under Parallels the loader may report a wrong GICR base and
    // reading it causes a Synchronous External Abort (DFSC=0x10).
    if gicr_base != 0
        && crate::arch_impl::aarch64::gic::GICR_VALID.load(core::sync::atomic::Ordering::Acquire)
    {
        unsafe {
            // GICR has RD_base (64KB) + SGI_base (64KB) per CPU
            // SGI_base is at offset 0x10000 from RD_base
            let gicr_rd = (HHDM + gicr_base) as *const u32;
            let gicr_waker = core::ptr::read_volatile(gicr_rd.byte_add(0x014));
            let gicr_sgi = (HHDM + gicr_base + 0x10000) as *const u32;
            let gicr_isenabler0 = core::ptr::read_volatile(gicr_sgi.byte_add(0x100));
            let gicr_ispendr0 = core::ptr::read_volatile(gicr_sgi.byte_add(0x200));
            let gicr_igroupr0 = core::ptr::read_volatile(gicr_sgi.byte_add(0x080));
            let ppi27_en = (gicr_isenabler0 >> 27) & 1;
            let ppi30_en = (gicr_isenabler0 >> 30) & 1;
            let ppi27_pend = (gicr_ispendr0 >> 27) & 1;
            let ppi30_pend = (gicr_ispendr0 >> 30) & 1;
            crate::serial_println!(
                "[timer] GICR@{:#x}: WAKER={:#x} ISENABLER0={:#010x} ISPENDR0={:#010x} IGROUPR0={:#010x}",
                gicr_base, gicr_waker, gicr_isenabler0, gicr_ispendr0, gicr_igroupr0);
            crate::serial_println!(
                "[timer] GICR PPI27: en={} pend={} | PPI30: en={} pend={}",
                ppi27_en,
                ppi27_pend,
                ppi30_en,
                ppi30_pend
            );

            // Read priority for PPIs 27 and 30
            // GICR_IPRIORITYR covers 4 IRQs per 32-bit register
            let prio_reg6 = core::ptr::read_volatile(gicr_sgi.byte_add(0x400 + 6 * 4)); // IRQs 24-27
            let prio_reg7 = core::ptr::read_volatile(gicr_sgi.byte_add(0x400 + 7 * 4)); // IRQs 28-31
            let prio27 = (prio_reg6 >> 24) & 0xFF;
            let prio30 = (prio_reg7 >> 16) & 0xFF;
            crate::serial_println!(
                "[timer] PPI27 priority={:#x} PPI30 priority={:#x}",
                prio27,
                prio30
            );
        }
    } else if gicr_base != 0 {
        crate::serial_println!(
            "[timer] GICR@{:#x}: skipped (not validated — probe failed or not yet run)",
            gicr_base
        );
    }
}

/// Raw serial output - no locks, single char for debugging (used by print_timer_count)
#[inline(always)]
fn raw_serial_char(c: u8) {
    crate::serial_aarch64::raw_serial_char(c);
}

/// Raw serial output - write a string without locks for debugging
#[allow(dead_code)] // Debug utility, kept for future use
#[inline(always)]
fn raw_serial_str(s: &[u8]) {
    crate::serial_aarch64::raw_serial_str(s);
}

/// Print a decimal number using raw serial output
/// Used by timer interrupt handler to output [TIMER_COUNT:N] markers
#[allow(dead_code)] // Debug utility, kept for future use
fn print_timer_count_decimal(count: u64) {
    if count == 0 {
        raw_serial_char(b'0');
    } else {
        // Convert to decimal digits (max u64 is 20 digits)
        let mut digits = [0u8; 20];
        let mut n = count;
        let mut i = 0;
        while n > 0 {
            digits[i] = (n % 10) as u8 + b'0';
            n /= 10;
            i += 1;
        }
        // Print in reverse order
        while i > 0 {
            i -= 1;
            raw_serial_char(digits[i]);
        }
    }
}

/// Print a u64 as 16-char zero-padded hexadecimal using raw serial output.
#[allow(dead_code)] // Debug utility for soft lockup dumps
fn print_hex_u64(val: u64) {
    const HEX: [u8; 16] = *b"0123456789abcdef";
    // Print 16 hex digits (big-endian nibble order)
    for i in (0..16).rev() {
        raw_serial_char(HEX[((val >> (i * 4)) & 0xF) as usize]);
    }
}

/// Check for soft lockup (CPU 0 only, called from timer interrupt).
///
/// Compares the current context switch count against the last observed value.
/// If no context switches have occurred for LOCKUP_THRESHOLD_TICKS timer
/// interrupts (~5 seconds), checks whether this is a real stall:
/// - If the scheduler lock is held → likely deadlock, report immediately
/// - If the ready queue is empty → single runnable thread, not a lockup
/// - If the ready queue has threads → scheduler is stuck, report
fn check_soft_lockup(current_tick: u64) {
    let ctx_count = crate::task::scheduler::context_switch_count();
    let last_ctx = WATCHDOG_LAST_CTX_SWITCH.load(Ordering::Relaxed);

    // Check context switch progress
    let ctx_progressed = ctx_count != last_ctx;
    if ctx_progressed {
        WATCHDOG_LAST_CTX_SWITCH.store(ctx_count, Ordering::Relaxed);
    }

    // Check syscall progress (system is alive if syscalls are being made)
    let syscall_count = crate::tracing::providers::counters::SYSCALL_TOTAL.aggregate();
    let last_syscall = WATCHDOG_LAST_SYSCALL.load(Ordering::Relaxed);
    let syscall_progressed = syscall_count != last_syscall;
    if syscall_progressed {
        WATCHDOG_LAST_SYSCALL.store(syscall_count, Ordering::Relaxed);
    }

    if ctx_progressed || syscall_progressed {
        // System is making progress — update baseline
        WATCHDOG_LAST_PROGRESS_TICK.store(current_tick, Ordering::Relaxed);
        WATCHDOG_REPORTED.store(false, Ordering::Relaxed);
        return;
    }

    // No progress on either metric — check how long
    let stall_start = WATCHDOG_LAST_PROGRESS_TICK.load(Ordering::Relaxed);
    if stall_start == 0 {
        // Not yet initialized
        WATCHDOG_LAST_PROGRESS_TICK.store(current_tick, Ordering::Relaxed);
        return;
    }

    let stall_ticks = current_tick.wrapping_sub(stall_start);
    if stall_ticks >= LOCKUP_THRESHOLD_TICKS && !WATCHDOG_REPORTED.load(Ordering::Relaxed) {
        // Always report stall — even if ready_queue is empty, userspace threads
        // might be stuck in BlockedOnTimer/Blocked state without being woken.
        // The dump includes per-thread state so we can diagnose the exact issue.
        WATCHDOG_REPORTED.store(true, Ordering::Relaxed);
        dump_lockup_state(stall_ticks);
    }
}

/// Dump diagnostic state when a soft lockup is detected.
/// Uses only lock-free serial output — safe to call from interrupt context.
fn dump_lockup_state(stall_ticks: u64) {
    raw_serial_str(b"\n\n!!! SOFT LOCKUP DETECTED !!!\n");
    raw_serial_str(b"No context switch for ~");
    print_timer_count_decimal(stall_ticks / TARGET_TIMER_HZ);
    raw_serial_str(b" seconds (");
    print_timer_count_decimal(stall_ticks);
    raw_serial_str(b" ticks)\n");

    // Try to get scheduler info without blocking (try_lock)
    // If the scheduler lock is held, that itself is diagnostic info
    raw_serial_str(b"Scheduler lock: ");
    // We use the global SCHEDULER directly via the public with_scheduler_try_lock helper
    if let Some(info) = crate::task::scheduler::try_dump_state() {
        raw_serial_str(b"acquired\n");
        raw_serial_str(b"  Ready queue length: ");
        print_timer_count_decimal(info.ready_queue_len);
        raw_serial_str(b"\n  Total threads: ");
        print_timer_count_decimal(info.total_threads);
        raw_serial_str(b"\n  Blocked threads: ");
        print_timer_count_decimal(info.blocked_count);

        // Per-CPU current/previous threads
        raw_serial_str(b"\n  Per-CPU state:\n");
        for cpu in 0..crate::arch_impl::aarch64::smp::MAX_CPUS.min(info.per_cpu_current.len()) {
            raw_serial_str(b"    CPU ");
            raw_serial_char(b'0' + cpu as u8);
            raw_serial_str(b": current=");
            print_timer_count_decimal(info.per_cpu_current[cpu]);
            raw_serial_str(b" previous=");
            print_timer_count_decimal(info.per_cpu_previous[cpu]);
            raw_serial_str(b"\n");
        }

        // Ready queue contents
        raw_serial_str(b"  Ready queue: [");
        for (i, &tid) in info.ready_queue_ids.iter().enumerate() {
            if i > 0 {
                raw_serial_str(b", ");
            }
            print_timer_count_decimal(tid);
        }
        raw_serial_str(b"]\n");

        // Per-thread state (state names: R=Ready, X=Running, B=Blocked, S=Signal, C=ChildExit, T=Timer, D=Terminated)
        raw_serial_str(b"  Threads:\n");
        for t in &info.threads {
            raw_serial_str(b"    tid=");
            print_timer_count_decimal(t.id);
            raw_serial_str(b" state=");
            let state_ch = match t.state {
                0 => b'R', // Ready
                1 => b'X', // Running
                2 => b'B', // Blocked
                3 => b'S', // BlockedOnSignal
                4 => b'C', // BlockedOnChildExit
                5 => b'T', // BlockedOnTimer
                6 => b'D', // Terminated
                _ => b'?',
            };
            raw_serial_char(state_ch);
            if t.blocked_in_syscall {
                raw_serial_str(b" bis");
            }
            if t.has_wake_time {
                raw_serial_str(b" wt");
            }
            if t.privilege == 1 {
                raw_serial_str(b" user");
            }
            raw_serial_str(b"\n");
        }
    } else {
        raw_serial_str(b"HELD (possible deadlock)\n");
    }

    // Try to get process manager info
    raw_serial_str(b"Process manager lock: ");
    if let Some(info) = crate::process::try_dump_state() {
        raw_serial_str(b"acquired\n");
        raw_serial_str(b"  Total processes: ");
        print_timer_count_decimal(info.total_processes);
        raw_serial_str(b"\n  Running: ");
        print_timer_count_decimal(info.running_count);
        raw_serial_str(b"\n  Blocked: ");
        print_timer_count_decimal(info.blocked_count);
        raw_serial_str(b"\n");
        // Dump individual process names and states
        for p in &info.processes {
            raw_serial_str(b"  PID ");
            print_timer_count_decimal(p.pid);
            raw_serial_str(b" [");
            raw_serial_str(p.state_str.as_bytes());
            raw_serial_str(b"] ");
            raw_serial_str(p.name.as_bytes());
            raw_serial_str(b"\n");
        }
    } else {
        raw_serial_str(b"HELD (possible deadlock)\n");
    }

    // Dump trace counters (lock-free atomics, always safe from interrupt context)
    dump_trace_counters();

    raw_serial_str(b"!!! END SOFT LOCKUP DUMP !!!\n\n");
}

/// Dump trace counter values using lock-free serial output.
/// Safe to call from interrupt context since TraceCounter uses per-CPU atomics.
fn dump_trace_counters() {
    use crate::tracing::providers::counters;

    raw_serial_str(b"Trace counters:\n");

    raw_serial_str(b"  SYSCALL_TOTAL:    ");
    print_timer_count_decimal(counters::SYSCALL_TOTAL.aggregate());
    raw_serial_str(b"\n  IRQ_TOTAL:        ");
    print_timer_count_decimal(counters::IRQ_TOTAL.aggregate());
    raw_serial_str(b"\n  CTX_SWITCH_TOTAL: ");
    print_timer_count_decimal(counters::CTX_SWITCH_TOTAL.aggregate());
    raw_serial_str(b"\n  TIMER_TICK_TOTAL: ");
    print_timer_count_decimal(counters::TIMER_TICK_TOTAL.aggregate());
    raw_serial_str(b"\n  FORK_TOTAL:       ");
    print_timer_count_decimal(counters::FORK_TOTAL.aggregate());
    raw_serial_str(b"\n  EXEC_TOTAL:       ");
    print_timer_count_decimal(counters::EXEC_TOTAL.aggregate());
    raw_serial_str(b"\n  Global ticks:     ");
    print_timer_count_decimal(crate::time::get_ticks());
    raw_serial_str(b"\n  Timer IRQ count:  ");
    print_timer_count_decimal(TIMER_INTERRUPT_COUNT.load(Ordering::Relaxed));
    raw_serial_str(b"\n");
}

/// Poll VirtIO keyboard and push characters to TTY
///
/// This routes keyboard input through the TTY subsystem for:
/// 1. Echo (so you can see what you type)
/// 2. Line discipline processing (backspace, Ctrl-C, etc.)
/// 3. Proper stdin delivery to userspace via TTY read
fn poll_keyboard_to_stdin() {
    use crate::drivers::virtio::input_mmio::{self, event_type};

    // Track shift state across calls
    static SHIFT_PRESSED: core::sync::atomic::AtomicBool =
        core::sync::atomic::AtomicBool::new(false);

    if !input_mmio::is_initialized() {
        return;
    }

    for event in input_mmio::poll_events() {
        if event.event_type == event_type::EV_KEY {
            let keycode = event.code;
            let pressed = event.value != 0;

            // Track modifier key state
            if input_mmio::is_shift(keycode) {
                SHIFT_PRESSED.store(pressed, core::sync::atomic::Ordering::Relaxed);
                continue;
            }

            // Only process key presses and repeats (not releases)
            if pressed {
                // Generate VT100 escape sequences for special keys
                // (F-keys, arrows, Home, End, Delete)
                if let Some(seq) = input_mmio::keycode_to_escape_seq(keycode) {
                    for &b in seq {
                        if !crate::tty::push_char_nonblock(b) {
                            crate::ipc::stdin::push_byte_from_irq(b);
                        }
                    }
                    continue;
                }

                let shift = SHIFT_PRESSED.load(core::sync::atomic::Ordering::Relaxed);
                if let Some(c) = input_mmio::keycode_to_char(keycode, shift) {
                    // Route through TTY for echo and line discipline processing.
                    // This is the non-blocking version safe for interrupt context.
                    if !crate::tty::push_char_nonblock(c as u8) {
                        // TTY busy - fall back to raw stdin buffer
                        // (no echo, but at least input isn't lost)
                        crate::ipc::stdin::push_byte_from_irq(c as u8);
                    }
                }
            }
        }
    }
}

/// Reset the quantum counter for the current CPU (called when switching threads)
pub fn reset_quantum() {
    #[cfg(feature = "boot_tests")]
    RESET_QUANTUM_CALL_COUNT.fetch_add(1, Ordering::SeqCst);
    let cpu_id = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
    let idx = if cpu_id < crate::arch_impl::aarch64::constants::MAX_CPUS {
        cpu_id
    } else {
        0
    };
    CURRENT_QUANTUM[idx].store(TIME_QUANTUM, Ordering::Relaxed);
}

/// Re-arm the current CPU's timer using the configured scheduling interval.
///
/// Context switches can return to kernel code that briefly masks IRQs before
/// finishing its critical section. Re-programming the per-CPU timer before the
/// resumed thread runs ensures the next tick is pending on this CPU even if the
/// thread immediately re-enters a short interrupt-masked region.
pub fn rearm_timer() {
    let ticks = TICKS_PER_INTERRUPT.load(Ordering::Relaxed);

    if crate::platform_config::is_vmware() {
        // VMware: re-arm both timers using CVAL (absolute compare value).
        // No IMASK dance needed here — rearm_timer is called from context switch
        // paths, not from interrupt handlers, so there's no pending interrupt to ack.
        unsafe {
            // Virtual timer
            let vcnt: u64;
            core::arch::asm!("mrs {}, cntvct_el0", out(reg) vcnt, options(nomem, nostack));
            let vcval = vcnt.wrapping_add(ticks);
            core::arch::asm!("msr cntv_cval_el0, {}", in(reg) vcval, options(nomem, nostack));
            core::arch::asm!("msr cntv_ctl_el0, {}", in(reg) 1u64, options(nomem, nostack));
            // Physical timer
            let pcnt: u64;
            core::arch::asm!("mrs {}, cntpct_el0", out(reg) pcnt, options(nomem, nostack));
            let pcval = pcnt.wrapping_add(ticks);
            core::arch::asm!("msr cntp_cval_el0, {}", in(reg) pcval, options(nomem, nostack));
            core::arch::asm!("msr cntp_ctl_el0, {}", in(reg) 1u64, options(nomem, nostack));
            core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
        }
    } else {
        arm_timer(ticks);
    }
}

/// Get reset_quantum() call count for tests.
#[cfg(feature = "boot_tests")]
pub fn reset_quantum_call_count() -> u64 {
    RESET_QUANTUM_CALL_COUNT.load(Ordering::SeqCst)
}

/// Reset reset_quantum() call count for tests.
#[cfg(feature = "boot_tests")]
pub fn reset_quantum_call_count_reset() {
    RESET_QUANTUM_CALL_COUNT.store(0, Ordering::SeqCst);
}

/// Initialize the timer on a secondary CPU.
///
/// Each CPU has its own virtual timer (PPI 27 is per-CPU). The distributor
/// does not need re-configuration for PPIs. We just arm the timer and enable
/// the interrupt in this CPU's GIC interface.
pub fn init_secondary() {
    // Arm the timer with the same interval as CPU 0
    let ticks = TICKS_PER_INTERRUPT.load(Ordering::Relaxed);
    arm_timer(ticks);

    // Enable the timer PPI in the GIC for this CPU.
    // PPIs are per-CPU, but ISENABLER0 (IRQs 0-31) is banked per-CPU,
    // so writing to it from this CPU enables it for this CPU.
    use crate::arch_impl::traits::InterruptController;
    crate::arch_impl::aarch64::gic::Gicv2::enable_irq(timer_irq() as u8);
}

/// Check if the timer is initialized
pub fn is_initialized() -> bool {
    TIMER_INITIALIZED.load(Ordering::Acquire)
}

/// Get the current CPU's quantum value (for debugging)
#[allow(dead_code)]
pub fn current_quantum() -> u32 {
    let cpu_id = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
    let idx = if cpu_id < crate::arch_impl::aarch64::constants::MAX_CPUS {
        cpu_id
    } else {
        0
    };
    CURRENT_QUANTUM[idx].load(Ordering::Relaxed)
}
