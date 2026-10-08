//! Boot-time interrupt-controller selection and lock-free EOI dispatch.
use super::{acpi, apic, ioapic};
use x86_64::instructions::port::Port;

/// Unknown PCI routing or unusable firmware retains legacy PIC delivery.
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
            && m.qemu_firmware
            && m.io_apic_count != 0
            && known_isa_intx_mapping()
            && (m.local_apic_address != 0 || m.local_apic_override.is_some())
    }) {
        let mode = apic::init(
            madt.local_apic_override
                .unwrap_or(u64::from(madt.local_apic_address)),
        );
        ioapic::init(&madt, apic::id());
        ioapic::set_enabled(1, true);
        ioapic::set_enabled(4, true);
        log::info!("Interrupt delivery: {} + IOAPIC; 8259 masked", mode);
    } else {
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

/// Stage 1 knows QEMU PC/PIIX's PCI-to-ISA wiring, not arbitrary ACPI _PRT.
/// The caller requires QEMU firmware as well as the i440FX/PIIX3 pair; a MADT alone says
/// nothing about a PCI function's config Interrupt Line versus its GSI.
fn known_isa_intx_mapping() -> bool {
    crate::drivers::pci::pci_read_config_dword(0, 0, 0, 0) == 0x1237_8086
        && crate::drivers::pci::pci_read_config_dword(0, 1, 0, 0) == 0x7000_8086
}
