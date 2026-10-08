//! MADT I/O APIC routing. Selector/window accesses are serialized outside IRQ context.
use super::acpi::{MadtCensus, MAX_IO_APICS};
use spin::Mutex;

use super::ioapic_route::{redirection, MASKED};
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
    assert!(destination < 255, "Invalid IOAPIC physical destination");
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
    for irq in (0u8..16).filter(|irq| *irq != 2) {
        // ISA IRQ2 is the PIC cascade, not a device line. IRQ0's override
        // commonly routes to GSI2; keep that masked for the LAPIC timer.
        let elcr_level = unsafe {
            use x86_64::instructions::port::Port;
            Port::<u8>::new(0x4d0 + u16::from(irq / 8)).read() & (1 << (irq % 8)) != 0
        };
        let source_override = madt.overrides[..madt.override_count]
            .iter()
            .find(|entry| entry.source == irq)
            .map(|entry| (entry.gsi, entry.flags));
        let (gsi, low, high) = redirection(irq, destination as u8, elcr_level, source_override);
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
            controller.write(0x11 + 2 * pin, high);
            controller.write(0x10 + 2 * pin, low);
        }
        let (actual_low, actual_high) = unsafe {
            (
                controller.read(0x10 + 2 * pin),
                controller.read(0x11 + 2 * pin),
            )
        };
        log::info!("IOAPIC route IRQ {} GSI {} override {:?} ELCR level={} low={:#010x} high={:#010x} remote_irr={} delivery_pending={} masked={}",
            irq, gsi, source_override, elcr_level, actual_low, actual_high,
            actual_low & (1 << 14) != 0, actual_low & (1 << 12) != 0, actual_low & MASKED != 0);
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
        let Some(route) = state.routes.get(irq as usize).copied().flatten() else {
            return;
        };
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
    if let Some((low, high)) = route_state(irq) {
        log::info!("IOAPIC IRQ {} enabled={} low={:#010x} high={:#010x} remote_irr={} delivery_pending={} masked={}",
            irq, enabled, low, high, low & (1 << 14) != 0, low & (1 << 12) != 0, low & MASKED != 0);
    }
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

/// Non-destructive redirection read, only for thread-side slow-I/O reporting.
pub fn route_state(irq: u8) -> Option<(u32, u32)> {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let state = STATE.lock();
        let route = state.routes.get(irq as usize).copied().flatten()?;
        let controller = state.controllers[route.controller];
        Some(unsafe {
            (
                controller.read(0x10 + 2 * route.pin),
                controller.read(0x11 + 2 * route.pin),
            )
        })
    })
}
