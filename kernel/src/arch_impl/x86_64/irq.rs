//! Boot-time interrupt-controller selection and lock-free EOI dispatch.
use super::{acpi, apic, ioapic};
use x86_64::instructions::port::Port;

/// A missing MADT/APIC uses the legacy controller. Refused firmware fails closed.
pub fn init(rsdp: Option<u64>, physical_offset: u64) {
    let madt = match acpi::read_madt(rsdp, physical_offset) {
        Ok(madt) => Some(madt),
        Err(acpi::MadtRefusal::NoRsdpFromBootloader | acpi::MadtRefusal::NoMadtInRootTable) => None,
        Err(reason) => panic!(
            "Cannot configure interrupts: {}",
            acpi::refusal_token(reason)
        ),
    };
    // Remap first and then fully mask both 8259s before switching controllers.
    unsafe {
        let mut pics = crate::interrupts::PICS.lock();
        pics.initialize();
        pics.write_masks(0xff, 0xff);
    }
    if let Some(madt) =
        madt.filter(|m| m.local_apic_address != 0 || m.local_apic_override.is_some())
    {
        let mode = apic::init(
            madt.local_apic_override
                .unwrap_or(u64::from(madt.local_apic_address)),
        );
        ioapic::init(&madt, apic::id());
        ioapic::set_enabled(1, true);
        ioapic::set_enabled(4, true);
        log::info!(
            "Interrupt delivery: {} + IOAPIC; LAPIC timer at 200 Hz; 8259 masked",
            mode
        );
    } else {
        set_enabled(0, true);
        set_enabled(1, true);
        set_enabled(4, true);
        log::info!("Interrupt delivery: PIC + PIT at 200 Hz (MADT reports no APIC)");
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
