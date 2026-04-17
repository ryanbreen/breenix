//! ARM64 GIC (Generic Interrupt Controller) - supports GICv2 and GICv3.
//!
//! The GIC has these components:
//! - GICD (Distributor): Routes interrupts to CPUs, manages priority/enable
//! - GICC (CPU Interface, GICv2): Per-CPU MMIO interface
//! - ICC (CPU Interface, GICv3): Per-CPU system register interface
//! - GICR (Redistributor, GICv3): Per-CPU SGI/PPI configuration
//!
//! Interrupt types:
//! - SGI (0-15): Software Generated Interrupts (IPIs)
//! - PPI (16-31): Private Peripheral Interrupts (per-CPU, e.g., timer)
//! - SPI (32-1019): Shared Peripheral Interrupts (global, e.g., devices)
//!
//! Hardware addresses are read from platform_config at runtime:
//! - QEMU virt: GICD=0x0800_0000, GICC=0x0801_0000 (GICv2)
//! - Parallels:  GICD=0x0201_0000, GICR=0x0250_0000 (GICv3)

#![allow(dead_code)]

use crate::arch_impl::traits::InterruptController;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

// =============================================================================
// Fault-tolerant MMIO probe support
// =============================================================================
//
// When probing unknown MMIO addresses (e.g. verifying the GICR base from the
// UEFI loader), a read from an unmapped or non-responding address causes a
// Synchronous External Abort (DFSC=0x10).  The DATA_ABORT handler in
// exception.rs checks MMIO_PROBE_ACTIVE and, when set, records the fault and
// advances ELR past the faulting instruction instead of crashing.
//
// Usage:
//   MMIO_PROBE_ACTIVE.store(true, SeqCst)
//   let val = unsafe { read_volatile(addr) }   // may fault
//   MMIO_PROBE_ACTIVE.store(false, SeqCst)
//   if MMIO_PROBE_FAULTED.load(SeqCst) { /* address is bad */ }

/// Set to true before a speculative MMIO read.  The DATA_ABORT handler
/// checks this flag and, when set, sets MMIO_PROBE_FAULTED and advances ELR
/// past the faulting instruction rather than crashing.
pub static MMIO_PROBE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Set to true by the DATA_ABORT handler when a fault occurs while
/// MMIO_PROBE_ACTIVE is true.  Reset to false before each probe attempt.
pub static MMIO_PROBE_FAULTED: AtomicBool = AtomicBool::new(false);

/// Whether the GICR base address has been validated as accessible.
/// Set to true after a successful probe of the GICR base.
/// All GICR MMIO accesses are skipped when false.
pub static GICR_VALID: AtomicBool = AtomicBool::new(false);

/// Whether the GICR base has already been probed (even if it failed).
static GICR_PROBED: AtomicBool = AtomicBool::new(false);

/// Attempt a 32-bit MMIO read using the fault-tolerant probe mechanism.
///
/// Returns `Some(value)` if the read succeeds, `None` if it triggers a
/// Synchronous External Abort (DFSC=0x10 / non-responding device).
///
/// SAFETY: The DATA_ABORT handler must be installed and must check
/// MMIO_PROBE_ACTIVE before this is called.  Only safe to call from
/// early-boot single-threaded context (CPU 0, no SMP yet).
pub fn probe_mmio_u32(phys_addr: usize) -> Option<u32> {
    let virt_addr = (GIC_HHDM + phys_addr) as *const u32;

    // Clear the fault flag before the attempt.
    MMIO_PROBE_FAULTED.store(false, Ordering::SeqCst);
    // Arm the probe — DATA_ABORT handler checks this.
    MMIO_PROBE_ACTIVE.store(true, Ordering::SeqCst);
    core::sync::atomic::fence(Ordering::SeqCst);

    let val = unsafe { core::ptr::read_volatile(virt_addr) };

    core::sync::atomic::fence(Ordering::SeqCst);
    // Disarm — we either got the value or the handler already set FAULTED.
    MMIO_PROBE_ACTIVE.store(false, Ordering::SeqCst);
    core::sync::atomic::fence(Ordering::SeqCst);

    if MMIO_PROBE_FAULTED.load(Ordering::SeqCst) {
        None
    } else {
        Some(val)
    }
}

/// Probe and validate the GICR base address.
///
/// Reads GICR_WAKER (offset 0x14) from the reported base.  If that faults,
/// tries a list of well-known fallback addresses.  If a responsive address is
/// found, updates platform_config and sets GICR_VALID.
///
/// Returns true if a valid GICR was found (either at the reported base or a
/// fallback), false if no valid GICR could be located.
fn probe_and_validate_gicr() -> bool {
    if GICR_PROBED.load(Ordering::Acquire) {
        return GICR_VALID.load(Ordering::Acquire);
    }
    GICR_PROBED.store(true, Ordering::Release);

    let reported_base = crate::platform_config::gicr_base_phys() as usize;

    // Candidate addresses to try in order:
    //   1. Whatever the UEFI loader reported (may be wrong on M5 Max)
    //   2. Known Parallels GICR on M3 Max
    //   3. Common alternative addresses seen on real Apple Silicon under Parallels
    let candidates: &[usize] = &[
        reported_base,
        0x0250_0000, // Parallels M3 Max
        0x0260_0000, // potential M5 Max offset
        0x0200_0000, // the base that causes the crash (wrong, but keep to log)
        0x080A_0000, // another common GICR location
    ];

    for &base in candidates {
        if base == 0 {
            continue;
        }
        // Read GICR_WAKER at offset 0x14.  A valid GICR responds with bits
        // [31:3] reserved-RAZ and bit[1] (ProcessorSleep) set on reset.
        // An absent device causes DFSC=0x10 (External Abort).
        let waker = probe_mmio_u32(base + GICR_WAKER);
        match waker {
            None => {
                crate::serial_println!(
                    "[gic] GICR probe {:#010x}: FAULT (Synchronous External Abort)",
                    base
                );
            }
            Some(0xFFFF_FFFF) => {
                crate::serial_println!("[gic] GICR probe {:#010x}: 0xFFFFFFFF (no device)", base);
            }
            Some(val) => {
                // Plausible GICR_WAKER value: bit[1]=ProcessorSleep is
                // typically set (1) at reset; bit[2]=ChildrenAsleep follows.
                // We accept any non-0xFFFFFFFF non-fault read as a valid GICR.
                crate::serial_println!(
                    "[gic] GICR probe {:#010x}: WAKER={:#010x} <<< VALID",
                    base,
                    val
                );
                // If this differs from what the loader reported, update it.
                if base != reported_base {
                    crate::serial_println!(
                        "[gic] GICR base mismatch: loader={:#010x}, using={:#010x}",
                        reported_base,
                        base
                    );
                    crate::platform_config::set_gicr_base_phys(base as u64);
                }
                GICR_VALID.store(true, Ordering::Release);
                return true;
            }
        }
    }

    crate::serial_println!(
        "[gic] WARNING: No valid GICR found at any known address. \
         GICR init will be skipped. Timer PPIs may not work."
    );
    false
}

// =============================================================================
// GIC Distributor (GICD) Register Offsets
// =============================================================================

/// Distributor Control Register
const GICD_CTLR: usize = 0x000;
/// Interrupt Controller Type Register
const GICD_TYPER: usize = 0x004;
/// Distributor Implementer Identification Register
const GICD_IIDR: usize = 0x008;
/// Interrupt Group Registers (1 bit per IRQ)
const GICD_IGROUPR: usize = 0x080;
/// Interrupt Set-Enable Registers (1 bit per IRQ)
const GICD_ISENABLER: usize = 0x100;
/// Interrupt Clear-Enable Registers (1 bit per IRQ)
const GICD_ICENABLER: usize = 0x180;
/// Interrupt Set-Pending Registers
const GICD_ISPENDR: usize = 0x200;
/// Interrupt Clear-Pending Registers
const GICD_ICPENDR: usize = 0x280;
/// Interrupt Set-Active Registers
const GICD_ISACTIVER: usize = 0x300;
/// Interrupt Clear-Active Registers
const GICD_ICACTIVER: usize = 0x380;
/// Interrupt Priority Registers (8 bits per IRQ)
const GICD_IPRIORITYR: usize = 0x400;
/// Interrupt Processor Targets Registers (8 bits per IRQ, SPI only)
const GICD_ITARGETSR: usize = 0x800;
/// Interrupt Configuration Registers (2 bits per IRQ)
const GICD_ICFGR: usize = 0xC00;

// =============================================================================
// GIC CPU Interface (GICC) Register Offsets
// =============================================================================

/// CPU Interface Control Register
const GICC_CTLR: usize = 0x000;
/// Interrupt Priority Mask Register
const GICC_PMR: usize = 0x004;
/// Binary Point Register
const GICC_BPR: usize = 0x008;
/// Interrupt Acknowledge Register
const GICC_IAR: usize = 0x00C;
/// End of Interrupt Register
const GICC_EOIR: usize = 0x010;
/// Running Priority Register
const GICC_RPR: usize = 0x014;
/// Highest Priority Pending Interrupt Register
const GICC_HPPIR: usize = 0x018;

// =============================================================================
// Constants
// =============================================================================

/// Number of IRQs per register (32 bits, 1 bit per IRQ)
const IRQS_PER_ENABLE_REG: u32 = 32;
/// Number of IRQs per priority register (4 IRQs, 8 bits each)
const IRQS_PER_PRIORITY_REG: u32 = 4;
/// Number of IRQs per target register (4 IRQs, 8 bits each)
const IRQS_PER_TARGET_REG: u32 = 4;

/// Default priority for all interrupts (lower = higher priority)
const DEFAULT_PRIORITY: u8 = 0xA0;
/// Priority mask threshold used by Linux for GIC CPU interface init.
const LINUX_DEFAULT_PMR: u8 = 0xF0;

/// Spurious interrupt ID (no pending interrupt)
const SPURIOUS_IRQ: u32 = 1023;
/// Maximum valid interrupt ID (GICv2: 0-1019 valid, 1020-1022 reserved, 1023 spurious)
const MAX_VALID_IRQ: u32 = 1019;

/// Whether GIC has been initialized
static GIC_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Whether all interrupts are Group 0 (VMware: IGROUPR0 is RAZ/WI).
/// When true, acknowledge via ICC_IAR0_EL1 and EOI via ICC_EOIR0_EL1.
static USE_GROUP0: AtomicBool = AtomicBool::new(false);

/// Whether GICD_CTLR.DS=1 (Disable Security) was successfully set.
/// When DS=0, Group 0 ICC system registers (ICC_BPR0_EL1, ICC_IGRPEN0_EL1,
/// ICC_IAR0_EL1, ICC_EOIR0_EL1) are UNDEFINED from NS EL1 and must NOT
/// be accessed. The NS CPU interface uses Group 1 registers exclusively;
/// the hypervisor/firmware maps Group 0 physical delivery to Group 1 NS.
static DS_ENABLED: AtomicBool = AtomicBool::new(false);

/// Tracks which group the last acknowledged interrupt came from.
/// 0 = Group 1 (normal), 1 = Group 0. Per-CPU would be ideal but
/// a single atomic suffices for the boot CPU (VMware is single-CPU for now).
static LAST_ACK_GROUP: AtomicU32 = AtomicU32::new(0);

/// Peripheral ID Register 2 (contains GIC architecture version)
const GICD_PIDR2: usize = 0xFE8;

// =============================================================================
// Register Access Helpers
// =============================================================================

/// HHDM base address for GIC MMIO access (compile-time constant).
const GIC_HHDM: usize = crate::arch_impl::aarch64::constants::HHDM_BASE as usize;

/// Active GIC version: 2 for GICv2, 3 for GICv3. Set during init.
static ACTIVE_GIC_VERSION: AtomicU8 = AtomicU8::new(0);

/// Read a 32-bit GICD register (address from platform_config).
#[inline]
fn gicd_read(offset: usize) -> u32 {
    unsafe {
        let base = crate::platform_config::gicd_base_phys() as usize;
        let addr = (GIC_HHDM + base + offset) as *const u32;
        core::ptr::read_volatile(addr)
    }
}

/// Write a 32-bit GICD register (address from platform_config).
#[inline]
fn gicd_write(offset: usize, value: u32) {
    unsafe {
        let base = crate::platform_config::gicd_base_phys() as usize;
        let addr = (GIC_HHDM + base + offset) as *mut u32;
        core::ptr::write_volatile(addr, value);
    }
}

/// Read a 32-bit GICC register (GICv2 only, address from platform_config).
#[inline]
fn gicc_read(offset: usize) -> u32 {
    unsafe {
        let base = crate::platform_config::gicc_base_phys() as usize;
        let addr = (GIC_HHDM + base + offset) as *const u32;
        core::ptr::read_volatile(addr)
    }
}

/// Write a 32-bit GICC register (GICv2 only, address from platform_config).
#[inline]
fn gicc_write(offset: usize, value: u32) {
    unsafe {
        let base = crate::platform_config::gicc_base_phys() as usize;
        let addr = (GIC_HHDM + base + offset) as *mut u32;
        core::ptr::write_volatile(addr, value);
    }
}

#[inline]
fn gicd_read_u64(offset: usize) -> u64 {
    let lo = gicd_read(offset) as u64;
    let hi = gicd_read(offset + 4) as u64;
    lo | (hi << 32)
}

#[inline]
fn raw_dump_prefix(intid: u32) {
    use crate::arch_impl::aarch64::context_switch::{raw_uart_dec, raw_uart_str};

    raw_uart_str("[STUCK_SPI");
    raw_uart_dec(intid as u64);
    raw_uart_str("] ");
}

#[inline]
fn raw_dump_u64(intid: u32, label: &str, value: u64) {
    use crate::arch_impl::aarch64::context_switch::{raw_uart_hex, raw_uart_str};

    raw_dump_prefix(intid);
    raw_uart_str(label);
    raw_uart_str("=");
    raw_uart_hex(value);
    raw_uart_str("\n");
}

#[inline]
fn raw_dump_bool(intid: u32, label: &str, value: bool) {
    use crate::arch_impl::aarch64::context_switch::raw_uart_str;

    raw_dump_prefix(intid);
    raw_uart_str(label);
    raw_uart_str("=");
    raw_uart_str(if value { "true" } else { "false" });
    raw_uart_str("\n");
}

#[inline]
fn raw_dump_note(intid: u32, label: &str, note: &str) {
    use crate::arch_impl::aarch64::context_switch::raw_uart_str;

    raw_dump_prefix(intid);
    raw_uart_str(label);
    raw_uart_str("=");
    raw_uart_str(note);
    raw_uart_str("\n");
}

/// Read a 32-bit GICR register (GICv3 only).
/// `cpu_offset` is the redistributor offset for this CPU (cpu * 0x20000).
///
/// Returns 0 if the GICR base has not yet been validated (GICR_VALID == false).
/// This prevents Synchronous External Aborts when the GICR probe has not run
/// yet (e.g., if a timer interrupt fires before init_gicv3_redistributor).
#[inline]
fn gicr_read(cpu_offset: usize, offset: usize) -> u32 {
    if !GICR_VALID.load(Ordering::Relaxed) {
        return 0;
    }
    unsafe {
        let base = crate::platform_config::gicr_base_phys() as usize;
        let addr = (GIC_HHDM + base + cpu_offset + offset) as *const u32;
        core::ptr::read_volatile(addr)
    }
}

/// Write a 32-bit GICR register (GICv3 only).
///
/// Silently drops the write if the GICR base has not yet been validated
/// (GICR_VALID == false). This prevents Synchronous External Aborts when
/// the GICR probe has not run yet.
#[inline]
fn gicr_write(cpu_offset: usize, offset: usize, value: u32) {
    if !GICR_VALID.load(Ordering::Relaxed) {
        return;
    }
    unsafe {
        let base = crate::platform_config::gicr_base_phys() as usize;
        let addr = (GIC_HHDM + base + cpu_offset + offset) as *mut u32;
        core::ptr::write_volatile(addr, value);
    }
}

// =============================================================================
// GICv2 Implementation
// =============================================================================

pub struct Gicv2;

impl Gicv2 {
    /// Get the number of supported IRQ lines
    fn num_irqs() -> u32 {
        let typer = gicd_read(GICD_TYPER);
        // ITLinesNumber field (bits 4:0) indicates (N+1)*32 interrupts
        ((typer & 0x1F) + 1) * 32
    }

    /// Initialize the GIC distributor
    fn init_distributor() {
        // Disable distributor while configuring
        gicd_write(GICD_CTLR, 0);

        let num_irqs = Self::num_irqs();

        // Disable all interrupts
        let num_regs = (num_irqs + 31) / 32;
        for i in 0..num_regs {
            gicd_write(GICD_ICENABLER + (i as usize * 4), 0xFFFF_FFFF);
        }

        // Clear all pending interrupts
        for i in 0..num_regs {
            gicd_write(GICD_ICPENDR + (i as usize * 4), 0xFFFF_FFFF);
        }

        // Configure all interrupts as Group 1 (delivered as IRQ, not FIQ)
        // IGROUPR: 1 bit per IRQ, 1 = Group 1 (IRQ), 0 = Group 0 (FIQ)
        // In non-secure world (where QEMU runs), Group 1 = IRQ, Group 0 = FIQ
        for i in 0..num_regs {
            gicd_write(GICD_IGROUPR + (i as usize * 4), 0xFFFF_FFFF);
        }

        // Set default priority for all interrupts
        let num_priority_regs = (num_irqs + 3) / 4;
        let priority_val = (DEFAULT_PRIORITY as u32) * 0x0101_0101; // Same priority in all 4 bytes
        for i in 0..num_priority_regs {
            gicd_write(GICD_IPRIORITYR + (i as usize * 4), priority_val);
        }

        // Route all SPIs to CPU 0 (target mask = 0x01)
        // SGIs and PPIs (0-31) have fixed targets
        let num_target_regs = (num_irqs + 3) / 4;
        for i in 8..num_target_regs {
            // Start at reg 8 (IRQ 32) for SPIs
            gicd_write(GICD_ITARGETSR + (i as usize * 4), 0x0101_0101);
        }

        // Configure all SPIs as level-triggered (default)
        // ICFGR has 2 bits per IRQ, 16 IRQs per register
        // Bit 1 of each pair: 0 = level, 1 = edge
        let num_cfg_regs = (num_irqs + 15) / 16;
        for i in 2..num_cfg_regs {
            // Start at reg 2 (IRQ 32) for SPIs
            gicd_write(GICD_ICFGR + (i as usize * 4), 0); // All level-triggered
        }

        // Enable distributor with both Group 0 and Group 1
        // Bit 0: EnableGrp0 (Group 0 forwarding)
        // Bit 1: EnableGrp1 (Group 1 forwarding)
        gicd_write(GICD_CTLR, 0x3);
    }

    /// Initialize the GIC CPU interface
    fn init_cpu_interface() {
        // Set priority mask to accept all priorities
        gicc_write(GICC_PMR, LINUX_DEFAULT_PMR as u32);

        // No preemption (binary point = 7 means all priority bits used for priority, none for subpriority)
        gicc_write(GICC_BPR, 7);

        // Enable CPU interface with both Group 0 and Group 1
        // Bit 0: EnableGrp0 (enable signaling of Group 0 interrupts)
        // Bit 1: EnableGrp1 (enable signaling of Group 1 interrupts)
        // Bit 2: AckCtl - When set, GICC_IAR can acknowledge both groups (important for non-secure)
        // Bit 3: FIQEn - When set, Group 0 interrupts are signaled as FIQ (we want IRQ for both)
        // Since we configured all interrupts as Group 1 in init_distributor(),
        // we need bit 1 set. Also set AckCtl to allow acknowledging any pending interrupt.
        gicc_write(GICC_CTLR, 0x7); // EnableGrp0 | EnableGrp1 | AckCtl
    }
}

/// Initialize the GIC CPU interface for a secondary CPU.
///
/// Dispatches to GICv2 or GICv3 based on the detected version.
pub fn init_cpu_interface_secondary() {
    let version = ACTIVE_GIC_VERSION.load(Ordering::Acquire);
    match version {
        2 => {
            gicc_write(GICC_PMR, LINUX_DEFAULT_PMR as u32);
            gicc_write(GICC_BPR, 7);
            gicc_write(GICC_CTLR, 0x7);
        }
        3 | 4 => {
            // Get CPU ID from MPIDR_EL1 for redistributor offset
            let cpu_id = get_cpu_id_from_mpidr();

            // ICC system registers are per-CPU and independent of the
            // redistributor MMIO aperture. Bring SRE/PMR/IGRPEN up before any
            // GICR range guard so every CPU emits the SRE audit line.
            init_gicv3_cpu_interface();

            if !validate_gicr_range_for_cpu(cpu_id) {
                return;
            }

            init_gicv3_redistributor(cpu_id);
        }
        _ => {}
    }
}

impl Gicv2 {
    /// Detect the GIC architecture version by reading GICD_PIDR2.
    ///
    /// The GICD base address (0x0800_0000) is present on both GICv2 and GICv3
    /// on QEMU virt, so this read is safe regardless of GIC version.
    /// Returns the architecture version (2, 3, or 4).
    fn detect_version() -> u8 {
        let pidr2 = gicd_read(GICD_PIDR2);
        ((pidr2 >> 4) & 0xF) as u8
    }
}

impl InterruptController for Gicv2 {
    /// Initialize the GIC - auto-detects v2 vs v3 and dispatches accordingly.
    fn init() {
        if GIC_INITIALIZED.load(Ordering::Relaxed) {
            return;
        }

        // Detect GIC version. Hardware detection (PIDR2) is authoritative.
        // Platform config can override (non-zero value), but defaults to 0
        // (auto-detect). Previously defaulted to 2, which masked GICv3 hardware
        // on Parallels and caused level-triggered SPI deactivation failures.
        let platform_version = crate::platform_config::gic_version();
        let hw_version = Self::detect_version();
        let version = if platform_version != 0 {
            platform_version
        } else {
            hw_version
        };

        ACTIVE_GIC_VERSION.store(version, Ordering::Release);

        match version {
            2 => {
                Self::init_distributor();
                Self::init_cpu_interface();
            }
            3 | 4 => {
                // Probe the GICR base address BEFORE any GICR MMIO access and
                // before enabling interrupts.  This ensures GICR_VALID is set
                // (or left false) prior to any interrupt handler running that
                // might indirectly call gicr_read/gicr_write.  The probe is
                // idempotent (guarded by GICR_PROBED), so calling it again
                // inside init_gicv3_redistributor(0) is a no-op.
                probe_and_validate_gicr();
                refresh_gicr_rdist_map(GICR_MAP_SLOTS, false);
                init_gicv3_distributor();
                init_gicv3_redistributor(0); // CPU 0
                init_gicv3_cpu_interface();
            }
            _ => {
                panic!("Unknown GIC architecture version {}", version);
            }
        }

        GIC_INITIALIZED.store(true, Ordering::Release);
    }

    /// Enable an IRQ
    fn enable_irq(irq: u8) {
        let irq_num = irq as u32;
        let version = ACTIVE_GIC_VERSION.load(Ordering::Relaxed);

        if irq_num < 32 {
            // SGI/PPI: on GICv3, use GICR; on GICv2, use GICD
            if version >= 3 {
                // Enable in GICR_ISENABLER0 for current CPU
                if !GICR_VALID.load(Ordering::Relaxed) {
                    return; // No valid GICR — skip
                }
                let cpu_id = get_cpu_id_from_mpidr();
                if !validate_gicr_range_for_cpu(cpu_id) {
                    return; // No redistributor for this CPU
                }
                if let Some(rd_base) = gicr_init_rd_base_for_cpu(cpu_id) {
                    gicr_write_at_rd_base(rd_base, GICR_SGI_OFFSET + GICR_ISENABLER0, 1 << irq_num);
                }
            } else {
                gicd_write(GICD_ISENABLER, 1 << irq_num);
            }
        } else {
            // SPI: always in GICD
            let reg_index = irq_num / IRQS_PER_ENABLE_REG;
            let bit = irq_num % IRQS_PER_ENABLE_REG;

            // When ARE (Affinity Routing Enable) is set in GICD_CTLR,
            // ITARGETSR is RAZ/WI — must use IROUTER even if the platform
            // reports GICv2. Parallels exposes GICv3 hardware with GICR
            // but the loader reports version 2.
            let ctlr = gicd_read(GICD_CTLR);
            let are_enabled = (ctlr & GICD_CTLR_ARE_NS) != 0;

            if version >= 3 || are_enabled {
                // GICv3 / ARE mode: Route SPI to current CPU via GICD_IROUTER
                let mpidr: u64;
                unsafe {
                    core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack));
                }
                let affinity = mpidr & 0xFF_00FF_FFFF;
                gicd_write(GICD_IROUTER + (irq_num as usize * 8), affinity as u32);
                gicd_write(
                    GICD_IROUTER + (irq_num as usize * 8) + 4,
                    (affinity >> 32) as u32,
                );
            } else {
                // GICv2 (no ARE): Route SPI to CPU 0 via ITARGETSR
                let target_reg = irq_num / 4;
                let target_byte = irq_num % 4;
                let current = gicd_read(GICD_ITARGETSR + (target_reg as usize * 4));
                let mask = 0xFFu32 << (target_byte * 8);
                let target_val = 0x01u32 << (target_byte * 8);
                gicd_write(
                    GICD_ITARGETSR + (target_reg as usize * 4),
                    (current & !mask) | target_val,
                );
            }

            gicd_write(GICD_ISENABLER + (reg_index as usize * 4), 1 << bit);
        }
    }

    /// Disable an IRQ
    fn disable_irq(irq: u8) {
        let irq_num = irq as u32;

        if irq_num < 32 {
            let version = ACTIVE_GIC_VERSION.load(Ordering::Relaxed);
            if version >= 3 {
                if !GICR_VALID.load(Ordering::Relaxed) {
                    return; // No valid GICR — skip
                }
                let cpu_id = get_cpu_id_from_mpidr();
                if !validate_gicr_range_for_cpu(cpu_id) {
                    return; // No redistributor for this CPU
                }
                if let Some(rd_base) = gicr_init_rd_base_for_cpu(cpu_id) {
                    gicr_write_at_rd_base(rd_base, GICR_SGI_OFFSET + GICR_ICENABLER0, 1 << irq_num);
                }
            } else {
                gicd_write(GICD_ICENABLER, 1 << irq_num);
            }
        } else {
            let reg_index = irq_num / IRQS_PER_ENABLE_REG;
            let bit = irq_num % IRQS_PER_ENABLE_REG;
            gicd_write(GICD_ICENABLER + (reg_index as usize * 4), 1 << bit);
        }
    }

    /// Signal End of Interrupt
    fn send_eoi(vector: u8) {
        // Write the interrupt ID to EOIR
        gicc_write(GICC_EOIR, vector as u32);
    }

    /// Get the IRQ offset (SPIs start at 32)
    fn irq_offset() -> u8 {
        32
    }
}

// =============================================================================
// Additional GIC Utilities
// =============================================================================

/// Acknowledge the current interrupt and get its ID.
///
/// Dispatches to GICv2 MMIO or GICv3 system registers.
/// For GICv3 with USE_GROUP0 (VMware), tries ICC_IAR0_EL1 first.
/// Returns the interrupt ID, or None if spurious (1023).
#[inline]
pub fn acknowledge_irq() -> Option<u32> {
    let version = ACTIVE_GIC_VERSION.load(Ordering::Relaxed);
    if version >= 3 {
        if USE_GROUP0.load(Ordering::Relaxed) && DS_ENABLED.load(Ordering::Relaxed) {
            // DS=1 path: Group 0 ICC registers are accessible.
            // Try ICC_IAR0_EL1 first (Group 0), fall back to IAR1.
            let iar0: u64;
            unsafe {
                core::arch::asm!("mrs {}, icc_iar0_el1", out(reg) iar0, options(nomem, nostack));
            }
            let id0 = (iar0 & 0xFFFFFF) as u32;
            if id0 <= MAX_VALID_IRQ {
                LAST_ACK_GROUP.store(0, Ordering::Relaxed); // Group 0
                return Some(id0);
            }
            // Fall through to try IAR1
            let iar1: u64;
            unsafe {
                core::arch::asm!("mrs {}, icc_iar1_el1", out(reg) iar1, options(nomem, nostack));
            }
            let id1 = (iar1 & 0xFFFFFF) as u32;
            if id1 <= MAX_VALID_IRQ {
                LAST_ACK_GROUP.store(1, Ordering::Relaxed); // Group 1
                return Some(id1);
            }
            None
        } else {
            // Normal GICv3 path (QEMU/Parallels): all interrupts are Group 1
            let iar: u64;
            unsafe {
                core::arch::asm!("mrs {}, icc_iar1_el1", out(reg) iar, options(nomem, nostack));
            }
            let irq_id = (iar & 0xFFFFFF) as u32;
            if irq_id > MAX_VALID_IRQ {
                None
            } else {
                LAST_ACK_GROUP.store(1, Ordering::Relaxed);
                Some(irq_id)
            }
        }
    } else {
        // GICv2: Read GICC_IAR
        let iar = gicc_read(GICC_IAR);
        let irq_id = iar & 0x3FF;
        if irq_id > MAX_VALID_IRQ {
            None
        } else {
            Some(irq_id)
        }
    }
}

/// Drop active IRQ priority by ID.
///
/// Uses EOIR0 for Group 0 interrupts (VMware) and EOIR1 for Group 1 (normal).
/// CRITICAL: `#[inline(never)]` prevents the compiler from hoisting the
/// GICC/ICC base into a callee-saved register shared with acknowledge_irq.
#[inline(never)]
pub fn priority_drop_irq(irq_id: u32) {
    let version = ACTIVE_GIC_VERSION.load(Ordering::Relaxed);
    if version >= 3 {
        let group = LAST_ACK_GROUP.load(Ordering::Relaxed);
        if group == 0 && DS_ENABLED.load(Ordering::Relaxed) {
            unsafe {
                core::arch::asm!("msr icc_eoir0_el1, {}", in(reg) irq_id as u64, options(nomem, nostack));
            }
        } else {
            unsafe {
                core::arch::asm!("msr icc_eoir1_el1, {}", in(reg) irq_id as u64, options(nomem, nostack));
            }
        }
    }
}

/// Deactivate an IRQ after handler completion.
#[inline(never)]
pub fn deactivate_irq(irq_id: u32) {
    let version = ACTIVE_GIC_VERSION.load(Ordering::Relaxed);
    if version >= 3 {
        unsafe {
            core::arch::asm!("msr icc_dir_el1, {}", in(reg) irq_id as u64, options(nomem, nostack));
        }
    } else {
        // GICv2: Write GICC_EOIR
        gicc_write(GICC_EOIR, irq_id);
    }
}

/// Signal end of interrupt by ID.
///
/// Retained as the combined helper for any paths that still want priority drop
/// and deactivation back-to-back.
#[inline(never)]
pub fn end_of_interrupt(irq_id: u32) {
    priority_drop_irq(irq_id);
    unsafe {
        core::arch::asm!("isb", options(nostack, preserves_flags));
    }
    deactivate_irq(irq_id);
}

/// Check if an IRQ is in Active state (GICD_ISACTIVER)
pub fn is_active(irq: u32) -> bool {
    let reg_index = irq / 32;
    let bit = irq % 32;
    let val = gicd_read(0x300 + (reg_index as usize * 4)); // GICD_ISACTIVER
    (val & (1 << bit)) != 0
}

/// Check if an IRQ is pending
pub fn is_pending(irq: u32) -> bool {
    let reg_index = irq / 32;
    let bit = irq % 32;
    let val = gicd_read(GICD_ISPENDR + (reg_index as usize * 4));
    (val & (1 << bit)) != 0
}

/// Set an IRQ to pending (software trigger)
pub fn set_pending(irq: u32) {
    let reg_index = irq / 32;
    let bit = irq % 32;
    gicd_write(GICD_ISPENDR + (reg_index as usize * 4), 1 << bit);
}

/// Clear a pending IRQ
pub fn clear_pending(irq: u32) {
    let reg_index = irq / 32;
    let bit = irq % 32;
    gicd_write(GICD_ICPENDR + (reg_index as usize * 4), 1 << bit);
}

/// Snapshot GICD_ISPENDR for SPIs 32-127 (registers 1-3).
///
/// Returns [ISPENDR1, ISPENDR2, ISPENDR3] covering SPIs 32-127.
/// Used by the AHCI driver to probe for platform wired interrupts:
/// for level-triggered SPIs, ISPENDR reflects the actual signal level
/// regardless of whether the SPI is enabled (GICv3 spec §8.9.16).
pub fn snapshot_pending_spis() -> [u32; 3] {
    [
        gicd_read(GICD_ISPENDR + 4),  // SPIs 32-63
        gicd_read(GICD_ISPENDR + 8),  // SPIs 64-95
        gicd_read(GICD_ISPENDR + 12), // SPIs 96-127
    ]
}

/// Dump a read-only GIC stuck-state snapshot for a pending SPI.
///
/// This is an observational diagnostic only: it performs volatile MMIO reads
/// and system-register reads but does not modify distributor or CPU-interface
/// state.
pub fn dump_stuck_state_for_spi(intid: u32) {
    use crate::arch_impl::aarch64::context_switch::{raw_uart_dec, raw_uart_hex, raw_uart_str};

    let cpu_id = get_cpu_id_from_mpidr() as u64;
    let version = ACTIVE_GIC_VERSION.load(Ordering::Relaxed);
    let reg_index = intid / 32;
    let bit_index = intid % 32;
    let ispendr_reg = gicd_read(GICD_ISPENDR + (reg_index as usize * 4));
    let isactiver_reg = gicd_read(GICD_ISACTIVER + (reg_index as usize * 4));
    let pending = (ispendr_reg & (1 << bit_index)) != 0;
    let active = (isactiver_reg & (1 << bit_index)) != 0;

    let icfgr_reg_index = intid / 16;
    let icfgr_field_shift = (intid % 16) * 2;
    let icfgr_reg = gicd_read(GICD_ICFGR + (icfgr_reg_index as usize * 4));
    let icfgr_bits = (icfgr_reg >> icfgr_field_shift) & 0x3;

    let ipriorityr_reg_index = intid / 4;
    let ipriorityr_byte_shift = (intid % 4) * 8;
    let ipriorityr_reg = gicd_read(GICD_IPRIORITYR + (ipriorityr_reg_index as usize * 4));
    let priority = (ipriorityr_reg >> ipriorityr_byte_shift) & 0xff;

    raw_dump_prefix(intid);
    raw_uart_str("cpu=");
    raw_uart_dec(cpu_id);
    raw_uart_str(" gic_version=");
    raw_uart_dec(version as u64);
    raw_uart_str("\n");

    raw_dump_prefix(intid);
    raw_uart_str("GICD_ISPENDR[");
    raw_uart_dec(reg_index as u64);
    raw_uart_str("]=");
    raw_uart_hex(ispendr_reg as u64);
    raw_uart_str(" bit=");
    raw_uart_dec(bit_index as u64);
    raw_uart_str(" pending=");
    raw_uart_str(if pending { "true" } else { "false" });
    raw_uart_str("\n");

    raw_dump_prefix(intid);
    raw_uart_str("GICD_ISACTIVER[");
    raw_uart_dec(reg_index as u64);
    raw_uart_str("]=");
    raw_uart_hex(isactiver_reg as u64);
    raw_uart_str(" bit=");
    raw_uart_dec(bit_index as u64);
    raw_uart_str(" active=");
    raw_uart_str(if active { "true" } else { "false" });
    raw_uart_str("\n");

    raw_dump_prefix(intid);
    raw_uart_str("GICD_ICFGR[");
    raw_uart_dec(icfgr_reg_index as u64);
    raw_uart_str("]=");
    raw_uart_hex(icfgr_reg as u64);
    raw_uart_str(" bits=");
    raw_uart_hex(icfgr_bits as u64);
    raw_uart_str(" trigger=");
    raw_uart_str(if (icfgr_bits & 0b10) != 0 {
        "edge"
    } else {
        "level"
    });
    raw_uart_str("\n");

    raw_dump_prefix(intid);
    raw_uart_str("GICD_IPRIORITYR[");
    raw_uart_dec(intid as u64);
    raw_uart_str("]=");
    raw_uart_hex(priority as u64);
    raw_uart_str("\n");

    let gicd_ctlr = gicd_read(GICD_CTLR);
    let are_enabled = (gicd_ctlr & GICD_CTLR_ARE_NS) != 0;
    if version >= 3 || are_enabled {
        let router = gicd_read_u64(GICD_IROUTER + (intid as usize * 8));
        raw_dump_prefix(intid);
        raw_uart_str("GICD_IROUTER[");
        raw_uart_dec(intid as u64);
        raw_uart_str("]=");
        raw_uart_hex(router);
        raw_uart_str("\n");
    } else {
        let target_reg_index = intid / 4;
        let target_byte_shift = (intid % 4) * 8;
        let itargetsr_reg = gicd_read(GICD_ITARGETSR + (target_reg_index as usize * 4));
        let target = (itargetsr_reg >> target_byte_shift) & 0xff;
        raw_dump_prefix(intid);
        raw_uart_str("GICD_ITARGETSR[");
        raw_uart_dec(intid as u64);
        raw_uart_str("]=");
        raw_uart_hex(target as u64);
        raw_uart_str("\n");
    }

    let current_el: u64;
    let daif: u64;
    unsafe {
        core::arch::asm!("mrs {}, currentel", out(reg) current_el, options(nomem, nostack));
        core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack));
    }

    let (rpr, pmr, bpr1, hppir1, ap1r0) = if version >= 3 {
        let rpr: u64;
        let pmr: u64;
        let bpr1: u64;
        let hppir1: u64;
        let ap1r0: u64;
        unsafe {
            core::arch::asm!("mrs {}, S3_0_C12_C11_3", out(reg) rpr, options(nomem, nostack));
            core::arch::asm!("mrs {}, icc_pmr_el1", out(reg) pmr, options(nomem, nostack));
            core::arch::asm!("mrs {}, icc_bpr1_el1", out(reg) bpr1, options(nomem, nostack));
            core::arch::asm!("mrs {}, icc_hppir1_el1", out(reg) hppir1, options(nomem, nostack));
            core::arch::asm!("mrs {}, icc_ap1r0_el1", out(reg) ap1r0, options(nomem, nostack));
        }
        (rpr, pmr, bpr1, hppir1, ap1r0)
    } else {
        (
            gicc_read(GICC_RPR) as u64,
            gicc_read(GICC_PMR) as u64,
            gicc_read(GICC_BPR) as u64,
            gicc_read(GICC_HPPIR) as u64,
            0,
        )
    };

    raw_dump_u64(intid, "ICC_RPR_EL1", rpr);
    raw_dump_u64(intid, "ICC_PMR_EL1", pmr);
    raw_dump_u64(intid, "ICC_BPR1_EL1", bpr1);
    raw_dump_u64(intid, "ICC_HPPIR1_EL1", hppir1);
    if version >= 3 {
        raw_dump_u64(intid, "ICC_AP1R0_EL1", ap1r0);
    } else {
        raw_dump_note(intid, "ICC_AP1R0_EL1", "unsupported_on_gicv2");
    }
    raw_dump_u64(intid, "DAIF", daif);
    if crate::per_cpu_aarch64::in_interrupt() {
        let spsr: u64;
        unsafe {
            core::arch::asm!("mrs {}, spsr_el1", out(reg) spsr, options(nomem, nostack));
        }
        raw_dump_u64(intid, "SPSR_EL1", spsr);
    } else {
        raw_dump_note(intid, "SPSR_EL1", "not_in_exception_context");
    }
    raw_dump_u64(intid, "CurrentEL", current_el);
    raw_dump_bool(intid, "peer_cpu_scan", false);
    raw_dump_note(
        intid,
        "peer_cpu_reason",
        "no_existing_ipi_callback_mechanism",
    );

    if intid == 34 {
        let mut sgi_targets = [0u8; 8];
        let target_count = crate::drivers::ahci::collect_recent_sgi_targets(&mut sgi_targets);
        for target in sgi_targets.iter().take(target_count) {
            dump_gicr_state_for_cpu(*target as usize);
        }

        raw_dump_prefix(intid);
        raw_uart_str("AHCI_PORT0_IS=");
        if let Some(port0_is) = crate::drivers::ahci::port0_is_snapshot() {
            raw_uart_hex(port0_is as u64);
        } else {
            raw_uart_str("unavailable");
        }
        raw_uart_str("\n");
        raw_dump_prefix(intid);
        raw_uart_str("AHCI_PORT1_IS=");
        if let Some(port1_is) = crate::drivers::ahci::port_is_snapshot(1) {
            raw_uart_hex(port1_is as u64);
        } else {
            raw_uart_str("unavailable");
        }
        raw_uart_str("\n");

        crate::drivers::ahci::dump_recent_ahci_events(None, 64);
    }
}

/// Send a Software Generated Interrupt (SGI) to a target CPU.
///
/// SGIs are interrupts 0-15 and are used for IPIs.
/// `target_cpu` is the CPU ID (0-7), NOT a bitmask.
pub fn send_sgi(sgi_id: u8, target_cpu: u8) {
    trace_sgi_boundary(crate::drivers::ahci::AHCI_TRACE_SGI_ENTRY, target_cpu);
    if sgi_id > 15 {
        trace_sgi_boundary(crate::drivers::ahci::AHCI_TRACE_SGI_EXIT, target_cpu);
        return;
    }

    let version = ACTIVE_GIC_VERSION.load(Ordering::Relaxed);
    if version >= 3 {
        unsafe {
            core::arch::asm!("dsb ishst", options(nomem, nostack));
        }

        // GICv3: Write ICC_SGI1R_EL1
        // Bits 55:48 = Aff3, 39:32 = Aff2, 23:16 = Aff1
        // Bits 15:0 = TargetList (bitmask within Aff1 group)
        // Bits 27:24 = INTID (SGI number)
        // For simple SMP (CPUs 0-7 in same affinity group):
        let target_list = 1u64 << (target_cpu as u64);
        trace_sgi_boundary(crate::drivers::ahci::AHCI_TRACE_SGI_AFTER_MPIDR, target_cpu);
        let sgir = ((sgi_id as u64) << 24) | target_list;
        trace_sgi_boundary(
            crate::drivers::ahci::AHCI_TRACE_SGI_AFTER_COMPOSE,
            target_cpu,
        );
        trace_sgi_boundary(crate::drivers::ahci::AHCI_TRACE_SGI_BEFORE_MSR, target_cpu);
        unsafe {
            core::arch::asm!("msr icc_sgi1r_el1, {}", in(reg) sgir, options(nomem, nostack));
        }
        trace_sgi_boundary(crate::drivers::ahci::AHCI_TRACE_SGI_AFTER_MSR, target_cpu);
        unsafe {
            core::arch::asm!("isb", options(nomem, nostack));
        }
        trace_sgi_boundary(crate::drivers::ahci::AHCI_TRACE_SGI_AFTER_ISB, target_cpu);
    } else {
        // GICv2: Write GICD_SGIR
        let target_mask = 1u32 << (target_cpu as u32);
        trace_sgi_boundary(crate::drivers::ahci::AHCI_TRACE_SGI_AFTER_MPIDR, target_cpu);
        let sgir = (target_mask << 16) | (sgi_id as u32);
        trace_sgi_boundary(
            crate::drivers::ahci::AHCI_TRACE_SGI_AFTER_COMPOSE,
            target_cpu,
        );
        trace_sgi_boundary(crate::drivers::ahci::AHCI_TRACE_SGI_BEFORE_MSR, target_cpu);
        gicd_write(0xF00, sgir);
        trace_sgi_boundary(crate::drivers::ahci::AHCI_TRACE_SGI_AFTER_MSR, target_cpu);
        trace_sgi_boundary(crate::drivers::ahci::AHCI_TRACE_SGI_AFTER_ISB, target_cpu);
    }
    trace_sgi_boundary(crate::drivers::ahci::AHCI_TRACE_SGI_EXIT, target_cpu);
}

#[inline(always)]
fn trace_sgi_boundary(site: u32, target_cpu: u8) {
    crate::drivers::ahci::push_ahci_event(site, 0, 0, 0, 0, 0, 0, target_cpu as u32, 0, false);
}

/// Check if GIC is initialized
#[inline]
pub fn is_initialized() -> bool {
    GIC_INITIALIZED.load(Ordering::Acquire)
}

/// Configure an SPI as edge-triggered (required for MSI interrupts).
///
/// GICD_ICFGR has 2 bits per IRQ: 0b00 = level, 0b10 = edge.
/// For IRQs 32+, register index = irq / 16, field = (irq % 16) * 2.
pub fn configure_spi_edge_triggered(irq: u32) {
    if irq < 32 {
        return; // Only SPIs (32+)
    }
    let reg_index = irq / 16;
    let field = (irq % 16) * 2;
    let current = gicd_read(GICD_ICFGR + (reg_index as usize * 4));
    // Set bit 1 of the 2-bit field to select edge-triggered
    let new_val = current | (0b10 << field);
    gicd_write(GICD_ICFGR + (reg_index as usize * 4), new_val);
}

/// Enable an SPI in the GIC distributor (GICD_ISENABLER).
///
/// Also routes the SPI to the current CPU via ITARGETSR (GICv2) or
/// IROUTER (GICv3). Only valid for IRQs >= 32 (SPIs).
///
/// Includes DSB+ISB to ensure the GICD write completes before returning.
/// Without this, the CPU write buffer may delay the enable, causing the
/// caller to miss an immediate pending interrupt.
pub fn enable_spi(irq: u32) {
    if irq < 32 {
        return; // Only SPIs (32+)
    }
    let version = ACTIVE_GIC_VERSION.load(Ordering::Relaxed);
    let reg_index = irq / 32;
    let bit = irq % 32;

    if version >= 3 {
        // GICv3: Route SPI to CPU 0 via GICD_IROUTER.
        // Use CPU 0's MPIDR affinity for explicit routing. 1-of-N mode
        // (IRM=1) is not reliably supported by all hypervisors.
        let mpidr: u64;
        unsafe {
            // Read boot CPU's MPIDR (we're always called from CPU 0 during init)
            core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack));
        }
        let affinity = mpidr & 0xFF_00FF_FFFF;
        gicd_write(GICD_IROUTER + (irq as usize * 8), affinity as u32);
        gicd_write(
            GICD_IROUTER + (irq as usize * 8) + 4,
            (affinity >> 32) as u32,
        );
    } else {
        // GICv2: Route SPI to CPU 0 via ITARGETSR
        let target_reg = irq / 4;
        let target_byte = irq % 4;
        let current = gicd_read(GICD_ITARGETSR + (target_reg as usize * 4));
        let mask = 0xFFu32 << (target_byte * 8);
        let target_val = 0x01u32 << (target_byte * 8);
        gicd_write(
            GICD_ITARGETSR + (target_reg as usize * 4),
            (current & !mask) | target_val,
        );
    }

    gicd_write(GICD_ISENABLER + (reg_index as usize * 4), 1 << bit);
    // DSB ensures the GICD write has completed (drained from write buffer)
    // before we return. ISB ensures subsequent instructions see the effect.
    unsafe {
        core::arch::asm!("dsb sy", options(nomem, nostack));
        core::arch::asm!("isb", options(nomem, nostack));
    }
}

/// Disable an SPI in the GIC distributor (GICD_ICENABLER).
///
/// Only valid for IRQs >= 32 (SPIs).
///
/// Includes DSB+ISB to ensure the GICD write has completed before
/// returning. Without this, subsequent MMIO operations (e.g., xHC ERDP
/// writes) could trigger new MSIs before the disable takes effect,
/// causing interrupt storms on virtual xHCI controllers.
pub fn disable_spi(irq: u32) {
    if irq < 32 {
        return; // Only SPIs (32+)
    }
    let reg_index = irq / 32;
    let bit = irq % 32;
    gicd_write(GICD_ICENABLER + (reg_index as usize * 4), 1 << bit);
    // DSB ensures the GICD write has completed (drained from write buffer)
    // before we return. This is critical in interrupt handlers where
    // subsequent MMIO to other devices (xHC) could generate new MSIs.
    unsafe {
        core::arch::asm!("dsb sy", options(nomem, nostack));
        core::arch::asm!("isb", options(nomem, nostack));
    }
}

/// Clear any pending state for an SPI (write-1-to-clear via GICD_ICPENDR).
pub fn clear_spi_pending(irq: u32) {
    if irq < 32 {
        return; // Only SPIs (32+)
    }
    let reg_index = irq / 32;
    let bit = irq % 32;
    gicd_write(GICD_ICPENDR + (reg_index as usize * 4), 1 << bit);
}

/// Debug function to dump GIC state for a specific IRQ.
pub fn dump_irq_state(irq: u32) {
    let reg_index = irq / 32;
    let bit_index = irq % 32;
    let version = ACTIVE_GIC_VERSION.load(Ordering::Relaxed);

    let isenabler = gicd_read(GICD_ISENABLER + (reg_index as usize * 4));
    let enabled = (isenabler & (1 << bit_index)) != 0;
    let igroupr = gicd_read(GICD_IGROUPR + (reg_index as usize * 4));
    let group1 = (igroupr & (1 << bit_index)) != 0;
    let ispendr = gicd_read(GICD_ISPENDR + (reg_index as usize * 4));
    let pending = (ispendr & (1 << bit_index)) != 0;

    let priority_reg_index = irq / 4;
    let priority_byte_index = irq % 4;
    let ipriorityr = gicd_read(GICD_IPRIORITYR + (priority_reg_index as usize * 4));
    let priority = ((ipriorityr >> (priority_byte_index * 8)) & 0xFF) as u8;

    let gicd_ctlr = gicd_read(GICD_CTLR);

    crate::serial_println!("[gic] IRQ {} state (GICv{}):", irq, version);
    crate::serial_println!(
        "  enabled={}, group1={}, pending={}",
        enabled,
        group1,
        pending
    );
    crate::serial_println!("  priority={:#x}, GICD_CTLR={:#x}", priority, gicd_ctlr);

    if version >= 3 {
        let pmr: u64;
        unsafe {
            core::arch::asm!("mrs {}, icc_pmr_el1", out(reg) pmr, options(nomem, nostack));
        }
        crate::serial_println!("  ICC_PMR={:#x}", pmr);
    } else {
        let gicc_ctlr = gicc_read(GICC_CTLR);
        let gicc_pmr = gicc_read(GICC_PMR);
        crate::serial_println!("  GICC_CTLR={:#x}, GICC_PMR={:#x}", gicc_ctlr, gicc_pmr);
    }
}

// =============================================================================
// GICv3 Constants and Initialization
// =============================================================================

/// GICR (Redistributor) register offsets.
/// Each redistributor has two 64KB frames: RD_base and SGI_base.
const GICR_FRAME_SIZE: usize = 0x2_0000; // 128KB per CPU (2 x 64KB frames)
const GICR_SGI_OFFSET: usize = 0x1_0000; // SGI_base is second 64KB frame
const GICR_TYPER_LAST: u64 = 1 << 4;
const GICR_MAP_SLOTS: usize = crate::arch_impl::aarch64::smp::MAX_CPUS;

/// GICR RD_base registers
const GICR_CTLR: usize = 0x000;
const GICR_WAKER: usize = 0x014;
const GICR_TYPER: usize = 0x008;
const GICR_SYNCR: usize = 0x0C0;

const LINUX_EXPECTED_SRE_ENABLED: u64 = 1;
const LINUX_EXPECTED_CTLR_EOIMODE: u64 = 1 << 1;
const LINUX_EXPECTED_PMR: u64 = 0xF0;
const LINUX_EXPECTED_IGRPEN1_ENABLED: u64 = 1;
const GIC_CPU_AUDIT_SLOTS: usize = 8;

static GIC_CPU_AUDIT_VALID: [AtomicBool; GIC_CPU_AUDIT_SLOTS] = [
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
];
static GIC_CPU_AUDIT_MPIDR: [AtomicU64; GIC_CPU_AUDIT_SLOTS] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];
static GIC_CPU_AUDIT_SRE: [AtomicU64; GIC_CPU_AUDIT_SLOTS] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];
static GIC_CPU_AUDIT_CTLR: [AtomicU64; GIC_CPU_AUDIT_SLOTS] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];
static GIC_CPU_AUDIT_PMR: [AtomicU64; GIC_CPU_AUDIT_SLOTS] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];
static GIC_CPU_AUDIT_IGRPEN1: [AtomicU64; GIC_CPU_AUDIT_SLOTS] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];
static GIC_CPU_AUDIT_MISMATCH: [AtomicU8; GIC_CPU_AUDIT_SLOTS] = [
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
];

static GICR_PER_CPU_BASE: [AtomicU64; GICR_MAP_SLOTS] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];
static GICR_PER_CPU_TYPER: [AtomicU64; GICR_MAP_SLOTS] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];
static GICR_PER_CPU_AFFINITY: [AtomicU64; GICR_MAP_SLOTS] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

/// GICR SGI_base registers (at GICR_SGI_OFFSET from RD_base)
const GICR_IGROUPR0: usize = 0x080;
const GICR_ISENABLER0: usize = 0x100;
const GICR_ICENABLER0: usize = 0x180;
const GICR_IPRIORITYR0: usize = 0x400;
const GICR_ICFGR0: usize = 0xC00;

/// GICD register for SPI routing (GICv3).
/// GICD_IROUTER[n] is at 0x6000 + n*8 per the GICv3 spec.
/// Previously this was 0x6100 (256 bytes too high), which wrote to the
/// wrong SPI's IROUTER. It "worked" because the reset default routes
/// all SPIs to CPU 0, so the wrong write was harmless. Fixed to match spec.
const GICD_IROUTER: usize = 0x6000;

/// GICv3 GICD_CTLR bits (Non-Secure register view, matching Linux irq-gic-v3.c)
const GICD_CTLR_ENABLE_GRP0: u32 = 1 << 0; // Enable Group 0 (RAZ/WI from NS when DS=0)
const GICD_CTLR_ENABLE_GRP1_NS: u32 = 1 << 1; // Enable Group 1 Non-Secure
const GICD_CTLR_ARE_NS: u32 = 1 << 4; // Affinity Routing Enable, NS (RAO/WI for GICv3)
const GICD_CTLR_DS: u32 = 1 << 6; // Disable Security (RAZ/WI from NS, but VMware may allow)

/// Get current CPU's linear ID from MPIDR_EL1.
/// For simple SMP, Aff0 is the CPU number.
fn get_cpu_id_from_mpidr() -> usize {
    let mpidr: u64;
    unsafe {
        core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack));
    }
    (mpidr & 0xFF) as usize
}

fn read_mpidr_el1() -> u64 {
    let mpidr: u64;
    unsafe {
        core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack));
    }
    mpidr
}

fn mpidr_to_gicr_affinity(mpidr: u64) -> u32 {
    let aff0 = mpidr & 0xff;
    let aff1 = (mpidr >> 8) & 0xff;
    let aff2 = (mpidr >> 16) & 0xff;
    let aff3 = (mpidr >> 32) & 0xff;
    ((aff3 << 24) | (aff2 << 16) | (aff1 << 8) | aff0) as u32
}

fn expected_gicr_affinity_for_cpu(cpu_id: usize) -> u32 {
    if cpu_id < GIC_CPU_AUDIT_SLOTS && GIC_CPU_AUDIT_VALID[cpu_id].load(Ordering::Acquire) {
        mpidr_to_gicr_affinity(GIC_CPU_AUDIT_MPIDR[cpu_id].load(Ordering::Acquire))
    } else {
        mpidr_to_gicr_affinity(cpu_id as u64)
    }
}

#[inline]
fn gicr_read_at_rd_base(rd_base_phys: usize, offset: usize) -> u32 {
    unsafe {
        let addr = (GIC_HHDM + rd_base_phys + offset) as *const u32;
        core::ptr::read_volatile(addr)
    }
}

#[inline]
fn gicr_write_at_rd_base(rd_base_phys: usize, offset: usize, value: u32) {
    unsafe {
        let addr = (GIC_HHDM + rd_base_phys + offset) as *mut u32;
        core::ptr::write_volatile(addr, value);
    }
}

#[inline]
fn gicr_read_u64_at_rd_base(rd_base_phys: usize, offset: usize) -> u64 {
    unsafe {
        let addr = (GIC_HHDM + rd_base_phys + offset) as *const u64;
        core::ptr::read_volatile(addr)
    }
}

#[inline]
fn gicr_rd_base_for_cpu(cpu_id: usize) -> Option<usize> {
    if cpu_id >= GICR_MAP_SLOTS {
        return None;
    }

    let rd_base = GICR_PER_CPU_BASE[cpu_id].load(Ordering::Acquire);
    if rd_base == 0 {
        None
    } else {
        Some(rd_base as usize)
    }
}

fn fallback_gicr_rd_base_for_cpu(cpu_id: usize) -> Option<usize> {
    let gicr_size = crate::platform_config::gicr_size() as usize;
    let max_redists = if gicr_size > 0 {
        gicr_size / GICR_FRAME_SIZE
    } else {
        0
    };
    if max_redists > 0 && cpu_id >= max_redists {
        None
    } else {
        Some(crate::platform_config::gicr_base_phys() as usize + cpu_id * GICR_FRAME_SIZE)
    }
}

fn gicr_init_rd_base_for_cpu(cpu_id: usize) -> Option<usize> {
    gicr_rd_base_for_cpu(cpu_id).or_else(|| fallback_gicr_rd_base_for_cpu(cpu_id))
}

fn validate_gicr_range_for_cpu(cpu_id: usize) -> bool {
    let gicr_size = crate::platform_config::gicr_size() as usize;
    let max_redists = if gicr_size > 0 {
        gicr_size / GICR_FRAME_SIZE
    } else {
        0
    };
    if max_redists > 0 && cpu_id >= max_redists {
        crate::serial_println!(
            "[gic] CPU {} (MPIDR Aff0) exceeds GICR region ({} redistributors), skipping GIC init",
            cpu_id,
            max_redists
        );
        return false;
    }

    true
}

fn refresh_gicr_rdist_map(max_cpus: usize, emit_logs: bool) {
    if !GICR_VALID.load(Ordering::Acquire) {
        if emit_logs {
            crate::serial_println!("[GICR_MAP] unavailable=gicr_not_valid");
        }
        return;
    }

    let target_cpus = max_cpus.min(GICR_MAP_SLOTS);
    for cpu_id in 0..target_cpus {
        GICR_PER_CPU_BASE[cpu_id].store(0, Ordering::Release);
        GICR_PER_CPU_TYPER[cpu_id].store(0, Ordering::Release);
        GICR_PER_CPU_AFFINITY[cpu_id].store(0, Ordering::Release);
    }

    let base = crate::platform_config::gicr_base_phys() as usize;
    let gicr_size = crate::platform_config::gicr_size() as usize;
    let max_frames = if gicr_size > 0 {
        (gicr_size / GICR_FRAME_SIZE).max(1)
    } else {
        GICR_MAP_SLOTS
    };

    for frame in 0..max_frames {
        let rd_base = base + frame * GICR_FRAME_SIZE;
        let typer = gicr_read_u64_at_rd_base(rd_base, GICR_TYPER);
        let affinity = (typer >> 32) as u32;

        for cpu_id in 0..target_cpus {
            if expected_gicr_affinity_for_cpu(cpu_id) == affinity {
                GICR_PER_CPU_BASE[cpu_id].store(rd_base as u64, Ordering::Release);
                GICR_PER_CPU_TYPER[cpu_id].store(typer, Ordering::Release);
                GICR_PER_CPU_AFFINITY[cpu_id].store(affinity as u64, Ordering::Release);
                break;
            }
        }

        if (typer & GICR_TYPER_LAST) != 0 {
            break;
        }
    }

    if emit_logs {
        for cpu_id in 0..target_cpus {
            let rd_base = GICR_PER_CPU_BASE[cpu_id].load(Ordering::Acquire);
            if rd_base == 0 {
                crate::serial_println!("[GICR_MAP] cpu={} NOT_FOUND", cpu_id);
            } else {
                crate::serial_println!(
                    "[GICR_MAP] cpu={} rd_base={:#x} typer={:#x} affinity={:#x}",
                    cpu_id,
                    rd_base,
                    GICR_PER_CPU_TYPER[cpu_id].load(Ordering::Acquire),
                    GICR_PER_CPU_AFFINITY[cpu_id].load(Ordering::Acquire)
                );
            }
        }
    }
}

/// Build and emit the Linux-style redistributor map.
///
/// Linux walks each 128KB redistributor frame and matches `GICR_TYPER[63:32]`
/// to the CPU affinity value; see irq-gic-v3.c `gic_populate_rdist()`.
pub fn init_gicr_rdist_map(max_cpus: usize) {
    refresh_gicr_rdist_map(max_cpus, true);
}

pub fn dump_gicr_state_for_cpu(target_cpu: usize) {
    use crate::arch_impl::aarch64::context_switch::{raw_uart_dec, raw_uart_hex, raw_uart_str};

    raw_uart_str("[GICR_STATE] cpu=");
    raw_uart_dec(target_cpu as u64);

    if !GICR_VALID.load(Ordering::Relaxed) {
        raw_uart_str(" unavailable=gicr_not_valid\n");
        return;
    }

    let Some(rd_base) = gicr_rd_base_for_cpu(target_cpu) else {
        raw_uart_str(" unavailable=rdist_not_found\n");
        return;
    };

    let waker = gicr_read_at_rd_base(rd_base, GICR_WAKER);
    let ctlr = gicr_read_at_rd_base(rd_base, GICR_CTLR);
    let typer = gicr_read_u64_at_rd_base(rd_base, GICR_TYPER);
    let syncr = gicr_read_at_rd_base(rd_base, GICR_SYNCR);

    raw_uart_str(" rd_base=");
    raw_uart_hex(rd_base as u64);
    raw_uart_str(" waker=");
    raw_uart_hex(waker as u64);
    raw_uart_str(" ctlr=");
    raw_uart_hex(ctlr as u64);
    raw_uart_str(" typer=");
    raw_uart_hex(typer);
    raw_uart_str(" syncr=");
    raw_uart_hex(syncr as u64);
    raw_uart_str("\n");
}

fn record_gic_cpu_audit(
    cpu_id: usize,
    mpidr: u64,
    sre: u64,
    ctlr: u64,
    pmr: u64,
    igrpen1: u64,
    mismatch: bool,
) {
    if cpu_id >= GIC_CPU_AUDIT_SLOTS {
        return;
    }

    GIC_CPU_AUDIT_MPIDR[cpu_id].store(mpidr, Ordering::Release);
    GIC_CPU_AUDIT_SRE[cpu_id].store(sre, Ordering::Release);
    GIC_CPU_AUDIT_CTLR[cpu_id].store(ctlr, Ordering::Release);
    GIC_CPU_AUDIT_PMR[cpu_id].store(pmr, Ordering::Release);
    GIC_CPU_AUDIT_IGRPEN1[cpu_id].store(igrpen1, Ordering::Release);
    GIC_CPU_AUDIT_MISMATCH[cpu_id].store(if mismatch { 1 } else { 0 }, Ordering::Release);
    GIC_CPU_AUDIT_VALID[cpu_id].store(true, Ordering::Release);
}

/// Emit CPU-interface audit values captured when each CPU initialized ICC state.
///
/// Secondary CPU boot.S emits raw UART breadcrumbs before Rust reaches the serial
/// lock, so printing these lines directly from each secondary can corrupt the
/// `[GIC_CPU_AUDIT]` prefix. This CPU0-owned snapshot keeps the required audit
/// rows parseable while preserving init-time register values.
pub fn dump_gic_cpu_audit_snapshot(max_cpus: usize) {
    let count = max_cpus.min(GIC_CPU_AUDIT_SLOTS);
    for cpu_id in 0..count {
        if !GIC_CPU_AUDIT_VALID[cpu_id].load(Ordering::Acquire) {
            crate::serial_println!("[GIC_CPU_AUDIT] cpu={} missing=1", cpu_id);
            continue;
        }

        crate::serial_println!(
            "[GIC_CPU_AUDIT] cpu={} mpidr={:#x} sre={:#x} ctlr={:#x} pmr={:#x} igrpen1={:#x} mismatch={}",
            cpu_id,
            GIC_CPU_AUDIT_MPIDR[cpu_id].load(Ordering::Acquire),
            GIC_CPU_AUDIT_SRE[cpu_id].load(Ordering::Acquire),
            GIC_CPU_AUDIT_CTLR[cpu_id].load(Ordering::Acquire),
            GIC_CPU_AUDIT_PMR[cpu_id].load(Ordering::Acquire),
            GIC_CPU_AUDIT_IGRPEN1[cpu_id].load(Ordering::Acquire),
            GIC_CPU_AUDIT_MISMATCH[cpu_id].load(Ordering::Acquire)
        );
    }
}

/// Initialize GICv3 Distributor (GICD).
fn init_gicv3_distributor() {
    // Disable distributor
    gicd_write(GICD_CTLR, 0);

    let num_irqs = {
        let typer = gicd_read(GICD_TYPER);
        ((typer & 0x1F) + 1) * 32
    };

    // Disable all SPIs
    let num_regs = (num_irqs + 31) / 32;
    for i in 1..num_regs {
        // Skip reg 0 (SGI/PPI, handled by GICR)
        gicd_write(GICD_ICENABLER + (i as usize * 4), 0xFFFF_FFFF);
    }

    // Clear all pending SPIs
    for i in 1..num_regs {
        gicd_write(GICD_ICPENDR + (i as usize * 4), 0xFFFF_FFFF);
    }

    // Set all SPIs to Group 1 Non-Secure
    for i in 1..num_regs {
        gicd_write(GICD_IGROUPR + (i as usize * 4), 0xFFFF_FFFF);
    }

    // Set default priority for all SPIs
    let num_priority_regs = (num_irqs + 3) / 4;
    let priority_val = (DEFAULT_PRIORITY as u32) * 0x0101_0101;
    for i in 8..num_priority_regs {
        // Skip first 8 regs (SGI/PPI priorities in GICR)
        gicd_write(GICD_IPRIORITYR + (i as usize * 4), priority_val);
    }

    // Try enabling DS=1 (Disable Security) first — VMware's emulation may allow it
    // even from NS EL1. If it works, IGROUPR0 becomes writable.
    gicd_write(
        GICD_CTLR,
        GICD_CTLR_DS | GICD_CTLR_ARE_NS | GICD_CTLR_ENABLE_GRP0 | GICD_CTLR_ENABLE_GRP1_NS,
    );
    unsafe {
        core::arch::asm!("dsb sy", options(nomem, nostack));
        core::arch::asm!("isb", options(nomem, nostack));
    }

    // Read back and log what stuck
    let ctlr_readback = gicd_read(GICD_CTLR);
    crate::serial_println!(
        "[gic] GICD_CTLR wrote {:#x}, readback={:#x}",
        GICD_CTLR_DS | GICD_CTLR_ARE_NS | GICD_CTLR_ENABLE_GRP0 | GICD_CTLR_ENABLE_GRP1_NS,
        ctlr_readback
    );

    // Track whether DS=1 took effect. When DS=0, Group 0 ICC system registers
    // (ICC_BPR0_EL1, ICC_IGRPEN0_EL1, ICC_IAR0_EL1, ICC_EOIR0_EL1) are
    // UNDEFINED from NS EL1 — accessing them causes a sync exception (EC=0x0).
    let ds_set = (ctlr_readback & GICD_CTLR_DS) != 0;
    DS_ENABLED.store(ds_set, Ordering::Release);
    if !ds_set {
        crate::serial_println!(
            "[gic] DS=0: Group 0 ICC regs inaccessible from NS EL1, using Group 1 only"
        );
    }
}

/// Initialize GICv3 Redistributor (GICR) for a specific CPU.
fn init_gicv3_redistributor(cpu_id: usize) {
    // On the first call (CPU 0), probe the GICR base address before touching
    // any GICR MMIO.  On M5 Max under Parallels the loader may report the
    // wrong base (0x02000000 instead of the real hardware address), causing a
    // Synchronous External Abort (DFSC=0x10) on the very first GICR read.
    if cpu_id == 0 {
        probe_and_validate_gicr();
    }

    // If the probe failed (or no valid GICR was found) skip all GICR MMIO.
    // GICv3 can deliver timer interrupts via ICC system registers alone on
    // some platforms; skipping GICR init degrades behaviour but won't crash.
    if !GICR_VALID.load(Ordering::Acquire) {
        crate::serial_println!(
            "[gic] CPU {} skipping GICR init (no valid GICR address found)",
            cpu_id
        );
        return;
    }

    let Some(rd_base) = gicr_init_rd_base_for_cpu(cpu_id) else {
        crate::serial_println!(
            "[gic] CPU {} has no redistributor frame, skipping GICR init",
            cpu_id
        );
        return;
    };

    // Wake up the redistributor
    let waker = gicr_read_at_rd_base(rd_base, GICR_WAKER);
    gicr_write_at_rd_base(rd_base, GICR_WAKER, waker & !(1 << 1)); // Clear ProcessorSleep

    // Wait for ChildrenAsleep to clear
    for _ in 0..10_000 {
        let w = gicr_read_at_rd_base(rd_base, GICR_WAKER);
        if (w & (1 << 2)) == 0 {
            break;
        }
        core::hint::spin_loop();
    }

    // Configure SGIs (0-15) and PPIs (16-31) in the redistributor

    // Set all SGI/PPI to Group 1
    gicr_write_at_rd_base(rd_base, GICR_SGI_OFFSET + GICR_IGROUPR0, 0xFFFF_FFFF);

    // Barrier to ensure the write completes before readback
    unsafe {
        core::arch::asm!("dsb sy", options(nomem, nostack));
        core::arch::asm!("isb", options(nomem, nostack));
    }

    // Read back IGROUPR0 to verify the write took effect
    let igroupr0 = gicr_read_at_rd_base(rd_base, GICR_SGI_OFFSET + GICR_IGROUPR0);

    if cpu_id == 0 {
        crate::serial_println!(
            "[gic] GICR_IGROUPR0: wrote 0xFFFFFFFF, readback={:#010x}",
            igroupr0
        );
    }

    if igroupr0 == 0 {
        // IGROUPR0 is RAZ/WI — security extensions prevent NS writes.
        // All interrupts are stuck in Group 0 (FIQ delivery).
        if cpu_id == 0 {
            USE_GROUP0.store(true, Ordering::Release);
            crate::serial_println!("[gic] IGROUPR0 RAZ/WI — all interrupts are Group 0 (FIQ)");
            if DS_ENABLED.load(Ordering::Acquire) {
                crate::serial_println!("[gic] DS=1: using ICC_IAR0/EOIR0 for Group 0 interrupts");
            } else {
                crate::serial_println!(
                    "[gic] DS=0: using ICC_IAR1/EOIR1 (hypervisor maps Group 0 to NS Group 1)"
                );
            }
        }
    }

    // Disable all SGI/PPI first
    gicr_write_at_rd_base(rd_base, GICR_SGI_OFFSET + GICR_ICENABLER0, 0xFFFF_FFFF);

    // Set default priority for SGIs and PPIs
    for i in 0..8u32 {
        let priority_val = (DEFAULT_PRIORITY as u32) * 0x0101_0101;
        gicr_write_at_rd_base(
            rd_base,
            GICR_SGI_OFFSET + GICR_IPRIORITYR0 + (i as usize * 4),
            priority_val,
        );
    }

    // Configure PPIs as level-triggered (default)
    gicr_write_at_rd_base(rd_base, GICR_SGI_OFFSET + GICR_ICFGR0, 0); // SGIs: always edge
    gicr_write_at_rd_base(rd_base, GICR_SGI_OFFSET + GICR_ICFGR0 + 4, 0); // PPIs: level-triggered
}

/// Initialize GICv3 CPU Interface via ICC system registers.
fn init_gicv3_cpu_interface() {
    unsafe {
        // Enable system register interface (ICC_SRE_EL1)
        let sre: u64 = 0x7; // SRE | DFB | DIB
        core::arch::asm!("msr icc_sre_el1, {}", in(reg) sre, options(nomem, nostack));
        core::arch::asm!("isb", options(nomem, nostack));

        // Explicitly set ICC_CTLR_EL1.EOImode = 1 (bit 1) so GICv3 uses split
        // priority-drop and deactivate semantics. This matches Linux's regular
        // gic_handle_irq() admission path: EOIR before handler dispatch, DIR
        // after the handler body completes.
        let icc_ctlr: u64;
        core::arch::asm!("mrs {}, icc_ctlr_el1", out(reg) icc_ctlr, options(nomem, nostack));
        let new_ctlr = icc_ctlr | (1u64 << 1);
        core::arch::asm!("msr icc_ctlr_el1, {}", in(reg) new_ctlr, options(nomem, nostack));
        core::arch::asm!("isb", options(nostack, preserves_flags));
        crate::serial_println!(
            "[gic] ICC_CTLR_EL1: {:#x} -> {:#x} (EOImode={})",
            icc_ctlr,
            new_ctlr,
            (new_ctlr >> 1) & 1
        );

        // Set priority mask to accept all (ICC_PMR_EL1)
        let pmr: u64 = LINUX_DEFAULT_PMR as u64;
        core::arch::asm!("msr icc_pmr_el1, {}", in(reg) pmr, options(nomem, nostack));

        // No preemption for Group 1 (ICC_BPR1_EL1)
        let bpr: u64 = 7;
        core::arch::asm!("msr icc_bpr1_el1, {}", in(reg) bpr, options(nomem, nostack));

        // Group 0 Binary Point Register (ICC_BPR0_EL1):
        // Only accessible from NS EL1 when DS=1. When DS=0, this register is
        // UNDEFINED and accessing it causes a sync exception (EC=0x0).
        if DS_ENABLED.load(Ordering::Acquire) {
            core::arch::asm!("msr icc_bpr0_el1, {}", in(reg) bpr, options(nomem, nostack));
        }

        // Enable Group 1 interrupts (ICC_IGRPEN1_EL1) — always accessible from NS EL1
        let grpen: u64 = 1;
        core::arch::asm!("msr icc_igrpen1_el1, {}", in(reg) grpen, options(nomem, nostack));

        // Group 0 Enable (ICC_IGRPEN0_EL1):
        // Only accessible from NS EL1 when DS=1. When DS=0, this register is
        // UNDEFINED. The hypervisor manages Group 0 enable on our behalf.
        if DS_ENABLED.load(Ordering::Acquire) {
            core::arch::asm!("msr icc_igrpen0_el1, {}", in(reg) grpen, options(nomem, nostack));
        }

        core::arch::asm!("isb", options(nomem, nostack));

        let sre_readback: u64;
        let ctlr_readback: u64;
        let pmr_readback: u64;
        let igrpen1_readback: u64;
        core::arch::asm!("mrs {}, icc_sre_el1", out(reg) sre_readback, options(nomem, nostack));
        core::arch::asm!("mrs {}, icc_ctlr_el1", out(reg) ctlr_readback, options(nomem, nostack));
        core::arch::asm!("mrs {}, icc_pmr_el1", out(reg) pmr_readback, options(nomem, nostack));
        core::arch::asm!("mrs {}, icc_igrpen1_el1", out(reg) igrpen1_readback, options(nomem, nostack));
        {
            let cpu_id = get_cpu_id_from_mpidr();
            let mpidr = read_mpidr_el1();
            let sre_enabled = sre_readback & 1;
            let igrpen1_enabled = igrpen1_readback & 1;
            let mismatch = sre_enabled != LINUX_EXPECTED_SRE_ENABLED
                || (ctlr_readback & LINUX_EXPECTED_CTLR_EOIMODE) != LINUX_EXPECTED_CTLR_EOIMODE
                || pmr_readback != LINUX_EXPECTED_PMR
                || igrpen1_enabled != LINUX_EXPECTED_IGRPEN1_ENABLED;

            record_gic_cpu_audit(
                cpu_id,
                mpidr,
                sre_readback,
                ctlr_readback,
                pmr_readback,
                igrpen1_readback,
                mismatch,
            );
        }
    }
}

/// Read GICR_ISENABLER0 for a specific CPU (SGI/PPI enable register).
/// Returns the 32-bit enable mask for interrupts 0-31 (SGIs 0-15, PPIs 16-31).
/// PPI 27 (timer) = bit 27.
pub fn read_gicr_isenabler0(cpu_id: usize) -> u32 {
    if !GICR_VALID.load(Ordering::Relaxed) {
        return 0xDEAD_BEEF; // sentinel: GICR not initialized
    }
    if let Some(rd_base) = gicr_rd_base_for_cpu(cpu_id) {
        gicr_read_at_rd_base(rd_base, GICR_SGI_OFFSET + GICR_ISENABLER0)
    } else {
        0xDEAD_BEEF
    }
}

/// Read GICR_ISPENDR0 for a specific CPU (SGI/PPI pending register).
/// Returns the 32-bit pending mask for interrupts 0-31 (SGIs 0-15, PPIs 16-31).
/// PPI 27 (timer) = bit 27.
pub fn read_gicr_ispendr0(cpu_id: usize) -> u32 {
    if !GICR_VALID.load(Ordering::Relaxed) {
        return 0xDEAD_BEEF; // sentinel: GICR not initialized
    }
    if let Some(rd_base) = gicr_rd_base_for_cpu(cpu_id) {
        gicr_read_at_rd_base(rd_base, GICR_SGI_OFFSET + 0x200) // ISPENDR0 offset
    } else {
        0xDEAD_BEEF
    }
}

/// Read GICR_ICENABLER0 for a specific CPU (SGI/PPI clear-enable register).
///
/// This is a read-only diagnostic snapshot. Architecturally, ICENABLER reads
/// return the enable state just like ISENABLER; writes clear enable bits.
pub fn read_gicr_icenabler0(cpu_id: usize) -> u32 {
    if !GICR_VALID.load(Ordering::Relaxed) {
        return 0xDEAD_BEEF; // sentinel: GICR not initialized
    }
    if let Some(rd_base) = gicr_rd_base_for_cpu(cpu_id) {
        gicr_read_at_rd_base(rd_base, GICR_SGI_OFFSET + GICR_ICENABLER0)
    } else {
        0xDEAD_BEEF
    }
}

/// Read the priority of a specific interrupt (0-31) from CPU 0's GICR.
/// Returns the 8-bit priority value (lower = higher priority).
pub fn read_gicr_priority_cpu0(irq: u32) -> u8 {
    if !GICR_VALID.load(Ordering::Relaxed) {
        return 0xFF;
    }
    let reg_index = (irq / 4) as usize;
    let byte_index = (irq % 4) as usize;
    let Some(rd_base) = gicr_rd_base_for_cpu(0) else {
        return 0xFF;
    };
    let reg_val = gicr_read_at_rd_base(rd_base, GICR_SGI_OFFSET + GICR_IPRIORITYR0 + reg_index * 4);
    ((reg_val >> (byte_index * 8)) & 0xFF) as u8
}

/// Get the active GIC version (for diagnostic purposes).
pub fn active_version() -> u8 {
    ACTIVE_GIC_VERSION.load(Ordering::Relaxed)
}

/// Whether all interrupts are Group 0 (VMware IGROUPR0 RAZ/WI).
pub fn use_group0() -> bool {
    USE_GROUP0.load(Ordering::Relaxed)
}

/// Whether GICD_CTLR.DS=1 was successfully set.
/// When false, Group 0 ICC system registers are inaccessible from NS EL1.
pub fn ds_enabled() -> bool {
    DS_ENABLED.load(Ordering::Relaxed)
}
