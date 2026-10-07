//! MADT I/O APIC routing. Selector/window accesses are serialized outside IRQ context.
use super::acpi::{MadtCensus, MAX_IO_APICS};
use spin::Mutex;

const MASKED: u32 = 1 << 16;
#[derive(Clone, Copy)]
struct Controller {
    base: usize,
    gsi: u32,
    pins: u32,
}
#[derive(Clone, Copy)]
struct Route {
    controller: usize,
    pin: u32,
    low: u32,
}
struct State {
    controllers: [Controller; MAX_IO_APICS],
    routes: [Option<Route>; 16],
}
static STATE: Mutex<State> = Mutex::new(State {
    controllers: [Controller {
        base: 0,
        gsi: 0,
        pins: 0,
    }; MAX_IO_APICS],
    routes: [None; 16],
});

impl Controller {
    unsafe fn read(self, register: u32) -> u32 {
        core::ptr::write_volatile(self.base as *mut u32, register);
        core::ptr::read_volatile((self.base + 0x10) as *const u32)
    }
    unsafe fn write(self, register: u32, value: u32) {
        core::ptr::write_volatile(self.base as *mut u32, register);
        core::ptr::write_volatile((self.base + 0x10) as *mut u32, value);
    }
}

pub fn init(madt: &MadtCensus, destination: u32) {
    assert!(
        destination < 255,
        "Invalid IOAPIC physical destination"
    );
    assert!(madt.io_apic_count != 0, "MADT has LAPIC but no IOAPIC");
    let mut state = STATE.lock();
    for (index, entry) in madt.io_apics[..madt.io_apic_count].iter().enumerate() {
        assert!(
            entry.address != 0 && entry.address & 0xfff == 0,
            "Invalid IOAPIC address"
        );
        let mut controller = Controller {
            base: crate::memory::map_mmio(u64::from(entry.address), 4096).expect("Map IOAPIC"),
            gsi: entry.gsi_base,
            pins: 0,
        };
        unsafe {
            controller.pins = ((controller.read(1) >> 16) & 0xff) + 1;
            assert!(
                controller.pins <= 120,
                "IOAPIC redirections exceed register space"
            );
            for pin in 0..controller.pins {
                controller.write(0x10 + 2 * pin, MASKED);
            }
        }
        state.controllers[index] = controller;
    }
    for irq in [0u8, 1, 4, 10, 11] {
        let mut gsi = u32::from(irq);
        // These are ISA IRQ numbers. ELCR preserves the firmware's PCI INTx
        // level setting when no MADT override specifies it; polarity is high.
        let mut low = u32::from(crate::interrupts::PIC_1_OFFSET + irq) | MASKED;
        unsafe {
            use x86_64::instructions::port::Port;
            if Port::<u8>::new(0x4d0 + u16::from(irq / 8)).read() & (1 << (irq % 8)) != 0 {
                low |= 1 << 15;
            }
        }
        for entry in &madt.overrides[..madt.override_count] {
            if entry.source == irq {
                gsi = entry.gsi;
                low &= !((1 << 13) | (1 << 15));
                if entry.flags & 3 == 3 {
                    low |= 1 << 13;
                }
                if (entry.flags >> 2) & 3 == 3 {
                    low |= 1 << 15;
                }
            }
        }
        let index = state.controllers[..madt.io_apic_count]
            .iter()
            .position(|c| gsi >= c.gsi && gsi - c.gsi < c.pins)
            .expect("No IOAPIC covers used IRQ GSI");
        let controller = state.controllers[index];
        let pin = gsi - controller.gsi;
        assert!(
            !state
                .routes
                .iter()
                .flatten()
                .any(|r| r.controller == index && r.pin == pin),
            "Used IRQs alias one GSI"
        );
        unsafe {
            controller.write(0x11 + 2 * pin, destination << 24);
            controller.write(0x10 + 2 * pin, low);
        }
        state.routes[irq as usize] = Some(Route {
            controller: index,
            pin,
            low,
        });
    }
}

pub fn set_enabled(irq: u8, enabled: bool) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let state = STATE.lock();
        let route = state
            .routes
            .get(irq as usize)
            .copied()
            .flatten()
            .expect("Unrouted IRQ");
        // IRQ0 is described and masked: scheduler ticks come from the LAPIC.
        let low = if enabled && irq != 0 {
            route.low & !MASKED
        } else {
            route.low | MASKED
        };
        unsafe {
            state.controllers[route.controller].write(0x10 + 2 * route.pin, low);
        }
    });
}

pub fn enabled(irq: u8) -> bool {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let state = STATE.lock();
        let Some(route) = state.routes.get(irq as usize).copied().flatten() else {
            return false;
        };
        unsafe { state.controllers[route.controller].read(0x10 + 2 * route.pin) & MASKED == 0 }
    })
}
