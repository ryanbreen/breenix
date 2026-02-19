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

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use crate::arch_impl::traits::InterruptController;

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
/// Priority mask: accept all priorities
const PRIORITY_MASK: u8 = 0xFF;

/// Spurious interrupt ID (no pending interrupt)
const SPURIOUS_IRQ: u32 = 1023;
/// Maximum valid interrupt ID (GICv2: 0-1019 valid, 1020-1022 reserved, 1023 spurious)
const MAX_VALID_IRQ: u32 = 1019;

/// Whether GIC has been initialized
static GIC_INITIALIZED: AtomicBool = AtomicBool::new(false);

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

/// Read a 32-bit GICR register (GICv3 only).
/// `cpu_offset` is the redistributor offset for this CPU (cpu * 0x20000).
#[inline]
fn gicr_read(cpu_offset: usize, offset: usize) -> u32 {
    unsafe {
        let base = crate::platform_config::gicr_base_phys() as usize;
        let addr = (GIC_HHDM + base + cpu_offset + offset) as *const u32;
        core::ptr::read_volatile(addr)
    }
}

/// Write a 32-bit GICR register (GICv3 only).
#[inline]
fn gicr_write(cpu_offset: usize, offset: usize, value: u32) {
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
        gicc_write(GICC_PMR, PRIORITY_MASK as u32);

        // No preemption (binary point = 7 means all priority bits used for priority, none for subpriority)
        gicc_write(GICC_BPR, 7);

        // Enable CPU interface with both Group 0 and Group 1
        // Bit 0: EnableGrp0 (enable signaling of Group 0 interrupts)
        // Bit 1: EnableGrp1 (enable signaling of Group 1 interrupts)
        // Bit 2: AckCtl - When set, GICC_IAR can acknowledge both groups (important for non-secure)
        // Bit 3: FIQEn - When set, Group 0 interrupts are signaled as FIQ (we want IRQ for both)
        // Since we configured all interrupts as Group 1 in init_distributor(),
        // we need bit 1 set. Also set AckCtl to allow acknowledging any pending interrupt.
        gicc_write(GICC_CTLR, 0x7);  // EnableGrp0 | EnableGrp1 | AckCtl
    }
}

/// Initialize the GIC CPU interface for a secondary CPU.
///
/// Dispatches to GICv2 or GICv3 based on the detected version.
pub fn init_cpu_interface_secondary() {
    let version = ACTIVE_GIC_VERSION.load(Ordering::Acquire);
    match version {
        2 => {
            gicc_write(GICC_PMR, PRIORITY_MASK as u32);
            gicc_write(GICC_BPR, 7);
            gicc_write(GICC_CTLR, 0x7);
        }
        3 | 4 => {
            // Get CPU ID from MPIDR_EL1 for redistributor offset
            let cpu_id = get_cpu_id_from_mpidr();
            init_gicv3_redistributor(cpu_id);
            init_gicv3_cpu_interface();
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

        // Detect GIC version from GICD_PIDR2 or platform_config.
        let platform_version = crate::platform_config::gic_version();
        let hw_version = Self::detect_version();
        let version = if platform_version != 0 { platform_version } else { hw_version };

        ACTIVE_GIC_VERSION.store(version, Ordering::Release);

        match version {
            2 => {
                Self::init_distributor();
                Self::init_cpu_interface();
            }
            3 | 4 => {
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
                let cpu_id = get_cpu_id_from_mpidr();
                let cpu_offset = cpu_id * GICR_FRAME_SIZE;
                let sgi_offset = GICR_SGI_OFFSET; // SGI_base frame within redistributor
                gicr_write(cpu_offset + sgi_offset, GICR_ISENABLER0, 1 << irq_num);
            } else {
                gicd_write(GICD_ISENABLER, 1 << irq_num);
            }
        } else {
            // SPI: always in GICD
            let reg_index = irq_num / IRQS_PER_ENABLE_REG;
            let bit = irq_num % IRQS_PER_ENABLE_REG;

            if version >= 3 {
                // GICv3: Route SPI to current CPU via GICD_IROUTER
                let mpidr: u64;
                unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack)); }
                let affinity = mpidr & 0xFF_00FF_FFFF; // Aff3.Aff2.Aff1.Aff0
                gicd_write(GICD_IROUTER + (irq_num as usize * 8), affinity as u32);
                gicd_write(GICD_IROUTER + (irq_num as usize * 8) + 4, (affinity >> 32) as u32);
            } else {
                // GICv2: Route SPI to CPU 0 via ITARGETSR
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
                let cpu_id = get_cpu_id_from_mpidr();
                let cpu_offset = cpu_id * GICR_FRAME_SIZE;
                gicr_write(cpu_offset + GICR_SGI_OFFSET, GICR_ICENABLER0, 1 << irq_num);
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
/// Returns the interrupt ID, or None if spurious (1023).
#[inline]
pub fn acknowledge_irq() -> Option<u32> {
    let version = ACTIVE_GIC_VERSION.load(Ordering::Relaxed);
    let irq_id = if version >= 3 {
        // GICv3: Read ICC_IAR1_EL1
        let iar: u64;
        unsafe { core::arch::asm!("mrs {}, icc_iar1_el1", out(reg) iar, options(nomem, nostack)); }
        (iar & 0xFFFFFF) as u32 // 24-bit INTID for GICv3
    } else {
        // GICv2: Read GICC_IAR
        let iar = gicc_read(GICC_IAR);
        iar & 0x3FF // 10-bit INTID for GICv2
    };

    if irq_id > MAX_VALID_IRQ {
        None
    } else {
        Some(irq_id)
    }
}

/// Signal end of interrupt by ID.
///
/// CRITICAL: `#[inline(never)]` prevents the compiler from hoisting the
/// GICC/ICC base into a callee-saved register shared with acknowledge_irq.
/// See original GICv2 comment for the full rationale.
#[inline(never)]
pub fn end_of_interrupt(irq_id: u32) {
    let version = ACTIVE_GIC_VERSION.load(Ordering::Relaxed);
    if version >= 3 {
        // GICv3: Write ICC_EOIR1_EL1
        unsafe {
            core::arch::asm!("msr icc_eoir1_el1, {}", in(reg) irq_id as u64, options(nomem, nostack));
        }
    } else {
        // GICv2: Write GICC_EOIR
        gicc_write(GICC_EOIR, irq_id);
    }
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

/// Send a Software Generated Interrupt (SGI) to a target CPU.
///
/// SGIs are interrupts 0-15 and are used for IPIs.
/// `target_cpu` is the CPU ID (0-7), NOT a bitmask.
pub fn send_sgi(sgi_id: u8, target_cpu: u8) {
    if sgi_id > 15 {
        return;
    }

    let version = ACTIVE_GIC_VERSION.load(Ordering::Relaxed);
    if version >= 3 {
        // GICv3: Write ICC_SGI1R_EL1
        // Bits 55:48 = Aff3, 39:32 = Aff2, 23:16 = Aff1
        // Bits 15:0 = TargetList (bitmask within Aff1 group)
        // Bits 27:24 = INTID (SGI number)
        // For simple SMP (CPUs 0-7 in same affinity group):
        let target_list = 1u64 << (target_cpu as u64);
        let sgir = ((sgi_id as u64) << 24) | target_list;
        unsafe {
            core::arch::asm!("msr icc_sgi1r_el1, {}", in(reg) sgir, options(nomem, nostack));
            core::arch::asm!("isb", options(nomem, nostack));
        }
    } else {
        // GICv2: Write GICD_SGIR
        let target_mask = 1u32 << (target_cpu as u32);
        let sgir = (target_mask << 16) | (sgi_id as u32);
        gicd_write(0xF00, sgir);
    }
}

/// Check if GIC is initialized
#[inline]
pub fn is_initialized() -> bool {
    GIC_INITIALIZED.load(Ordering::Acquire)
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
    crate::serial_println!("  enabled={}, group1={}, pending={}", enabled, group1, pending);
    crate::serial_println!("  priority={:#x}, GICD_CTLR={:#x}", priority, gicd_ctlr);

    if version >= 3 {
        let pmr: u64;
        unsafe { core::arch::asm!("mrs {}, icc_pmr_el1", out(reg) pmr, options(nomem, nostack)); }
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

/// GICR RD_base registers
const GICR_CTLR: usize = 0x000;
const GICR_WAKER: usize = 0x014;
const GICR_TYPER: usize = 0x008;

/// GICR SGI_base registers (at GICR_SGI_OFFSET from RD_base)
const GICR_IGROUPR0: usize = 0x080;
const GICR_ISENABLER0: usize = 0x100;
const GICR_ICENABLER0: usize = 0x180;
const GICR_IPRIORITYR0: usize = 0x400;
const GICR_ICFGR0: usize = 0xC00;

/// GICD register for SPI routing (GICv3)
const GICD_IROUTER: usize = 0x6100;

/// GICv3 GICD_CTLR bits
const GICD_CTLR_ARE_NS: u32 = 1 << 4; // Affinity Routing Enable (Non-Secure)
const GICD_CTLR_ENABLE_GRP1_NS: u32 = 1 << 1; // Enable Group 1 Non-Secure

/// Get current CPU's linear ID from MPIDR_EL1.
/// For simple SMP, Aff0 is the CPU number.
fn get_cpu_id_from_mpidr() -> usize {
    let mpidr: u64;
    unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack)); }
    (mpidr & 0xFF) as usize
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

    // Enable distributor with ARE_NS and Group 1 NS
    gicd_write(GICD_CTLR, GICD_CTLR_ARE_NS | GICD_CTLR_ENABLE_GRP1_NS);
}

/// Initialize GICv3 Redistributor (GICR) for a specific CPU.
fn init_gicv3_redistributor(cpu_id: usize) {
    let cpu_offset = cpu_id * GICR_FRAME_SIZE;

    // Wake up the redistributor
    let waker = gicr_read(cpu_offset, GICR_WAKER);
    gicr_write(cpu_offset, GICR_WAKER, waker & !(1 << 1)); // Clear ProcessorSleep

    // Wait for ChildrenAsleep to clear
    for _ in 0..10_000 {
        let w = gicr_read(cpu_offset, GICR_WAKER);
        if (w & (1 << 2)) == 0 {
            break;
        }
        core::hint::spin_loop();
    }

    let sgi_base = cpu_offset + GICR_SGI_OFFSET;

    // Configure SGIs (0-15) and PPIs (16-31) in the redistributor

    // Set all SGI/PPI to Group 1
    gicr_write(sgi_base, GICR_IGROUPR0, 0xFFFF_FFFF);

    // Disable all SGI/PPI first
    gicr_write(sgi_base, GICR_ICENABLER0, 0xFFFF_FFFF);

    // Set default priority for SGIs and PPIs
    for i in 0..8u32 {
        let priority_val = (DEFAULT_PRIORITY as u32) * 0x0101_0101;
        gicr_write(sgi_base, GICR_IPRIORITYR0 + (i as usize * 4), priority_val);
    }

    // Configure PPIs as level-triggered (default)
    gicr_write(sgi_base, GICR_ICFGR0, 0); // SGIs: always edge
    gicr_write(sgi_base, GICR_ICFGR0 + 4, 0); // PPIs: level-triggered
}

/// Initialize GICv3 CPU Interface via ICC system registers.
fn init_gicv3_cpu_interface() {
    unsafe {
        // Enable system register interface (ICC_SRE_EL1)
        let sre: u64 = 0x7; // SRE | DFB | DIB
        core::arch::asm!("msr icc_sre_el1, {}", in(reg) sre, options(nomem, nostack));
        core::arch::asm!("isb", options(nomem, nostack));

        // Set priority mask to accept all (ICC_PMR_EL1)
        let pmr: u64 = PRIORITY_MASK as u64;
        core::arch::asm!("msr icc_pmr_el1, {}", in(reg) pmr, options(nomem, nostack));

        // No preemption (ICC_BPR1_EL1)
        let bpr: u64 = 7;
        core::arch::asm!("msr icc_bpr1_el1, {}", in(reg) bpr, options(nomem, nostack));

        // Enable Group 1 interrupts (ICC_IGRPEN1_EL1)
        let grpen: u64 = 1;
        core::arch::asm!("msr icc_igrpen1_el1, {}", in(reg) grpen, options(nomem, nostack));

        core::arch::asm!("isb", options(nomem, nostack));
    }
}

/// Get the active GIC version (for diagnostic purposes).
pub fn active_version() -> u8 {
    ACTIVE_GIC_VERSION.load(Ordering::Relaxed)
}
