//! ARM64 GICv2 (Generic Interrupt Controller) interrupt controller.
//!
//! The GICv2 has two main components:
//! - GICD (Distributor): Routes interrupts to CPUs, manages priority/enable
//! - GICC (CPU Interface): Per-CPU interface for acknowledging/completing IRQs
//!
//! Interrupt types:
//! - SGI (0-15): Software Generated Interrupts (IPIs)
//! - PPI (16-31): Private Peripheral Interrupts (per-CPU, e.g., timer)
//! - SPI (32-1019): Shared Peripheral Interrupts (global, e.g., devices)
//!
//! For QEMU virt machine:
//! - GICD base: 0x0800_0000
//! - GICC base: 0x0801_0000

#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, Ordering};
use crate::arch_impl::traits::InterruptController;
use crate::arch_impl::aarch64::constants::{GICD_BASE, GICC_BASE};

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

/// Whether GIC has been initialized
static GIC_INITIALIZED: AtomicBool = AtomicBool::new(false);

// =============================================================================
// Register Access Helpers
// =============================================================================

/// Read a 32-bit GICD register
#[inline]
fn gicd_read(offset: usize) -> u32 {
    unsafe {
        let addr = (GICD_BASE as usize + offset) as *const u32;
        core::ptr::read_volatile(addr)
    }
}

/// Write a 32-bit GICD register
#[inline]
fn gicd_write(offset: usize, value: u32) {
    unsafe {
        let addr = (GICD_BASE as usize + offset) as *mut u32;
        core::ptr::write_volatile(addr, value);
    }
}

/// Read a 32-bit GICC register
#[inline]
fn gicc_read(offset: usize) -> u32 {
    unsafe {
        let addr = (GICC_BASE as usize + offset) as *const u32;
        core::ptr::read_volatile(addr)
    }
}

/// Write a 32-bit GICC register
#[inline]
fn gicc_write(offset: usize, value: u32) {
    unsafe {
        let addr = (GICC_BASE as usize + offset) as *mut u32;
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

        // Enable distributor
        gicd_write(GICD_CTLR, 1);
    }

    /// Initialize the GIC CPU interface
    fn init_cpu_interface() {
        // Set priority mask to accept all priorities
        gicc_write(GICC_PMR, PRIORITY_MASK as u32);

        // No preemption (binary point = 7 means all priority bits used for priority, none for subpriority)
        gicc_write(GICC_BPR, 7);

        // Enable CPU interface
        gicc_write(GICC_CTLR, 1);
    }
}

impl InterruptController for Gicv2 {
    /// Initialize the GIC
    fn init() {
        if GIC_INITIALIZED.load(Ordering::Relaxed) {
            return;
        }

        Self::init_distributor();
        Self::init_cpu_interface();

        GIC_INITIALIZED.store(true, Ordering::Release);
    }

    /// Enable an IRQ
    fn enable_irq(irq: u8) {
        let irq = irq as u32;
        let reg_index = irq / IRQS_PER_ENABLE_REG;
        let bit = irq % IRQS_PER_ENABLE_REG;

        // Write 1 to ISENABLER to enable (writes of 0 have no effect)
        gicd_write(GICD_ISENABLER + (reg_index as usize * 4), 1 << bit);
    }

    /// Disable an IRQ
    fn disable_irq(irq: u8) {
        let irq = irq as u32;
        let reg_index = irq / IRQS_PER_ENABLE_REG;
        let bit = irq % IRQS_PER_ENABLE_REG;

        // Write 1 to ICENABLER to disable (writes of 0 have no effect)
        gicd_write(GICD_ICENABLER + (reg_index as usize * 4), 1 << bit);
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

/// Acknowledge the current interrupt and get its ID
///
/// Returns the interrupt ID, or None if spurious.
#[inline]
pub fn acknowledge_irq() -> Option<u32> {
    let iar = gicc_read(GICC_IAR);
    let irq_id = iar & 0x3FF; // Bits 9:0 are the interrupt ID

    if irq_id == SPURIOUS_IRQ {
        None
    } else {
        Some(irq_id)
    }
}

/// Signal end of interrupt by ID
#[inline]
pub fn end_of_interrupt(irq_id: u32) {
    gicc_write(GICC_EOIR, irq_id);
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

/// Send a Software Generated Interrupt (SGI) to a target CPU
///
/// SGIs are interrupts 0-15 and are used for IPIs.
pub fn send_sgi(sgi_id: u8, target_cpu: u8) {
    if sgi_id > 15 {
        return;
    }

    // GICD_SGIR format:
    // Bits 25:24 = TargetListFilter (0 = use target list)
    // Bits 23:16 = CPUTargetList (bitmask of target CPUs)
    // Bits 3:0 = SGIINTID (SGI number)
    let sgir = ((target_cpu as u32) << 16) | (sgi_id as u32);
    gicd_write(0xF00, sgir); // GICD_SGIR offset
}

/// Check if GIC is initialized
#[inline]
pub fn is_initialized() -> bool {
    GIC_INITIALIZED.load(Ordering::Acquire)
}
