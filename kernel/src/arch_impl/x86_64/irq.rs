//! Boot-time interrupt-controller selection and lock-free EOI dispatch.
//!
//! The MADT decides the controller on each firmware: when it describes an
//! I/O APIC and a local APIC (and no structure this kernel cannot honour), the
//! ISA interrupts are routed through the I/O APIC with the MADT's source
//! overrides, the local APICs take over the timer, and the 8259s are masked.
//! Otherwise the 8259 PIC and the PIT deliver, on the boot CPU only.
//!
//! Which I/O APIC input a PCI function's INTx line drives is a separate
//! question the MADT does not answer (that is ACPI `_PRT`, which this kernel
//! does not interpret). `pci_intx_routing_known` answers it only for the QEMU
//! chipsets whose wiring is fixed; elsewhere a PCI device interrupts by MSI.
use core::sync::atomic::{AtomicBool, Ordering};

use super::{acpi, apic, ioapic};
use x86_64::instructions::port::Port;

static PCI_INTX_ROUTING_KNOWN: AtomicBool = AtomicBool::new(false);

/// Whether a PCI function's config-space Interrupt Line names the ISA IRQ, and
/// with the I/O APIC active the GSI, its INTx actually drives. True only on
/// QEMU's PC/PIIX and Q35/ICH9 machines (`known_isa_intx_mapping`); a driver
/// that needs an interrupt on other firmware uses MSI.
pub fn pci_intx_routing_known() -> bool {
    PCI_INTX_ROUTING_KNOWN.load(Ordering::Acquire)
}

/// Choose the interrupt controller from the MADT and route the ISA interrupts.
pub fn init(rsdp: Option<u64>, physical_offset: u64) {
    let madt = match acpi::read_madt(rsdp, physical_offset) {
        Ok(madt) => Some(madt),
        Err(acpi::MadtRefusal::NoRsdpFromBootloader | acpi::MadtRefusal::NoMadtInRootTable) => None,
        Err(reason) => {
            log::warn!(
                "Interrupt topology unavailable: {}; retaining PIC",
                acpi::refusal_token(reason)
            );
            None
        }
    };
    // Remap first and then fully mask both 8259s before switching controllers.
    unsafe {
        let mut pics = crate::interrupts::PICS.lock();
        pics.initialize();
        pics.write_masks(0xff, 0xff);
    }
    if let Some(madt) = madt.filter(|m| {
        m.interrupt_topology_supported
            && m.io_apic_count != 0
            && (m.local_apic_address != 0 || m.local_apic_override.is_some())
    }) {
        // The Interrupt Line is a GSI only with the routing the QEMU
        // chipsets fix; QEMU's firmware is checked too, as before.
        PCI_INTX_ROUTING_KNOWN.store(
            madt.qemu_firmware && known_isa_intx_mapping(),
            Ordering::Release,
        );
        apic::set_local_nmis(&madt);
        let mode = apic::init(
            madt.local_apic_override
                .unwrap_or(u64::from(madt.local_apic_address)),
        );
        ioapic::init(&madt, apic::id());
        ioapic::set_enabled(1, true);
        ioapic::set_enabled(4, true);
        log::info!("Interrupt delivery: {} + IOAPIC; 8259 masked", mode);
        log::info!(
            "[irq] I/O APICs={} source overrides={} local NMI entries={} PCI INTx routing {}",
            madt.io_apic_count,
            madt.override_count,
            madt.local_nmi_count,
            if pci_intx_routing_known() {
                "known (QEMU chipset)"
            } else {
                "unknown: PCI devices interrupt by MSI"
            }
        );
    } else {
        // With the PIC, an Interrupt Line names the 8259 input directly on the
        // QEMU chipsets, whose firmware programs PIRQ routing to that line.
        PCI_INTX_ROUTING_KNOWN.store(known_isa_intx_mapping(), Ordering::Release);
        set_enabled(0, true);
        set_enabled(1, true);
        set_enabled(4, true);
        log::info!("Interrupt delivery: PIC + PIT (APIC/PCI routing unavailable or unsupported)");
    }
    unsafe {
        let mut status = Port::<u8>::new(0x64);
        let mut data = Port::<u8>::new(0x60);
        for _ in 0..10 {
            if status.read() & 1 == 0 {
                break;
            }
            let _ = data.read();
        }
    }
}

pub fn set_enabled(irq: u8, enabled: bool) {
    if apic::active() {
        ioapic::set_enabled(irq, enabled);
    } else {
        assert!(irq < 16, "Invalid PIC IRQ");
        x86_64::instructions::interrupts::without_interrupts(|| unsafe {
            let mut pics = crate::interrupts::PICS.lock();
            let mut masks = pics.read_masks();
            if enabled {
                masks[(irq / 8) as usize] &= !(1 << (irq % 8));
                if irq >= 8 {
                    masks[0] &= !(1 << 2);
                }
            } else {
                masks[(irq / 8) as usize] |= 1 << (irq % 8);
            }
            pics.write_masks(masks[0], masks[1]);
        });
    }
}

pub fn enabled(irq: u8) -> bool {
    if apic::active() {
        ioapic::enabled(irq)
    } else {
        x86_64::instructions::interrupts::without_interrupts(|| unsafe {
            let masks = crate::interrupts::PICS.lock().read_masks();
            irq < 16 && masks[(irq / 8) as usize] & (1 << (irq % 8)) == 0
        })
    }
}

/// Hot path: acknowledge the executing CPU, never wait on a contended lock.
#[inline]
pub fn eoi(vector: u8) {
    if apic::active() {
        apic::eoi();
    } else {
        unsafe {
            if let Some(mut pics) = crate::interrupts::PICS.try_lock() {
                pics.notify_end_of_interrupt(vector);
            } else if (32..48).contains(&vector) {
                if vector >= 40 {
                    Port::<u8>::new(0xa0).write(0x20);
                }
                Port::<u8>::new(0x20).write(0x20);
            }
        }
    }
}

/// Stage 1 knows QEMU's PCI-to-ISA wiring for PC/PIIX and Q35/ICH9, not
/// arbitrary ACPI _PRT. On both, a PCI interrupt the chipset routes to ISA IRQ
/// n (the function's config Interrupt Line, as the firmware programmed the
/// PIRQ routing) drives the I/O APIC input with GSI n as well as the 8259's.
/// The caller requires QEMU firmware as well as the i440FX/PIIX3 or MCH/ICH9
/// LPC pair; a MADT alone says nothing about an Interrupt Line versus its GSI.
fn known_isa_intx_mapping() -> bool {
    let read = crate::drivers::pci::pci_read_config_dword;
    let piix = read(0, 0, 0, 0) == 0x1237_8086 && read(0, 1, 0, 0) == 0x7000_8086;
    let ich9 = read(0, 0, 0, 0) == 0x29c0_8086 && read(0, 31, 0, 0) == 0x2918_8086;
    piix || ich9
}
