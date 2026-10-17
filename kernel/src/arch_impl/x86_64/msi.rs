//! Message-signalled interrupts on x86-64, delivered through the local APIC.
//!
//! A PCI function with an MSI capability is given one vector from
//! `FIRST_VECTOR..FIRST_VECTOR + VECTORS` and programmed (PCI Local Bus
//! specification 3.0, 6.8.1) to write that vector to one processor's local
//! APIC. Each vector's IDT gate (`entry`) runs the driver's handler with no
//! lock and acknowledges the local APIC. This needs no knowledge of how the
//! platform wires INTx, which is why drivers on firmware other than QEMU's use
//! it (`irq::pci_intx_routing_known`).
//!
//! Messages go to one processor, the one the I/O APIC routes go to
//! (`set_destination`), so device interrupts land on the same CPU either way.
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use x86_64::structures::idt::InterruptStackFrame;

use crate::drivers::pci::{pci_read_config_word, pci_write_config_dword, pci_write_config_word, Device};

/// The first MSI vector, and how many there are. Above the I/O APIC's ISA
/// vectors (0x20-0x2f), below the system call gate (0x80).
pub const FIRST_VECTOR: u8 = 0x50;
pub const VECTORS: usize = 16;

/// Each vector's handler as a `fn()` address; 0 while the vector is free.
static HANDLERS: [AtomicUsize; VECTORS] = [const { AtomicUsize::new(0) }; VECTORS];
/// The function programmed with each vector: `bus << 16 | device << 11 |
/// function << 8 | capability offset`, or `NO_DEVICE`.
static DEVICES: [AtomicU32; VECTORS] = [const { AtomicU32::new(NO_DEVICE) }; VECTORS];
const NO_DEVICE: u32 = u32::MAX;
/// The APIC id messages are addressed to.
static DESTINATION: AtomicU32 = AtomicU32::new(0);

// MSI capability layout (PCI Local Bus 3.0, 6.8.1).
const CONTROL: u8 = 2;
const ADDRESS: u8 = 4;
const CONTROL_ENABLE: u16 = 1 << 0;
const CONTROL_MULTIPLE_ENABLE: u16 = 0b111 << 4;
const CONTROL_64BIT: u16 = 1 << 7;
const CONTROL_MASKABLE: u16 = 1 << 8;
const COMMAND: u8 = 0x04;
const COMMAND_INTX_DISABLE: u16 = 1 << 10;

/// Whether MSI can be used: the local APIC is active and the FADT does not
/// forbid it.
pub fn available() -> bool {
    super::apic::active() && super::platform::msi_allowed()
}

/// Program `device`'s MSI capability to raise one vector whose gate runs
/// `handler`, and turn its INTx off. Returns the vector.
pub fn enable(device: &Device, handler: fn()) -> Result<u8, &'static str> {
    if !available() {
        return Err("MSI unavailable: no local APIC, or the FADT forbids it");
    }
    let capability = device.find_msi_capability().ok_or("no MSI capability")?;
    let index = (0..VECTORS)
        .find(|&index| {
            HANDLERS[index]
                .compare_exchange(0, handler as usize, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        })
        .ok_or("no free MSI vector")?;
    let vector = FIRST_VECTOR + index as u8;
    let Some((address, data)) =
        super::msi_message::compose(vector, DESTINATION.load(Ordering::Acquire))
    else {
        HANDLERS[index].store(0, Ordering::Release);
        return Err("MSI destination APIC id does not fit the message");
    };
    let (bus, slot, function) = (device.bus, device.device, device.function);
    let control = pci_read_config_word(bus, slot, function, capability + CONTROL);
    let wide = control & CONTROL_64BIT != 0;
    let data_at = capability + if wide { 12 } else { 8 };
    // Off while it is written, one message, then address, data and unmask.
    let off = control & !(CONTROL_ENABLE | CONTROL_MULTIPLE_ENABLE);
    pci_write_config_word(bus, slot, function, capability + CONTROL, off);
    pci_write_config_dword(bus, slot, function, capability + ADDRESS, address);
    if wide {
        pci_write_config_dword(bus, slot, function, capability + ADDRESS + 4, 0);
    }
    pci_write_config_word(bus, slot, function, data_at, data as u16);
    if control & CONTROL_MASKABLE != 0 {
        pci_write_config_dword(bus, slot, function, data_at + 4, 0);
    }
    let command = pci_read_config_word(bus, slot, function, COMMAND);
    pci_write_config_word(bus, slot, function, COMMAND, command | COMMAND_INTX_DISABLE);
    DEVICES[index].store(
        (u32::from(bus) << 16) | (u32::from(slot) << 11) | (u32::from(function) << 8) | u32::from(capability),
        Ordering::Release,
    );
    pci_write_config_word(bus, slot, function, capability + CONTROL, off | CONTROL_ENABLE);
    Ok(vector)
}

/// Address each programmed message, and later ones, to the processor with
/// APIC id `destination`. The message address is one 32-bit configuration
/// write, so a device cannot see half of it.
pub fn set_destination(destination: u32) {
    let Some((address, _)) = super::msi_message::compose(FIRST_VECTOR, destination) else {
        return;
    };
    DESTINATION.store(destination, Ordering::Release);
    for device in DEVICES.iter().map(|device| device.load(Ordering::Acquire)) {
        if device == NO_DEVICE {
            continue;
        }
        let (bus, slot, function) = ((device >> 16) as u8, ((device >> 11) & 0x1f) as u8, ((device >> 8) & 7) as u8);
        pci_write_config_dword(bus, slot, function, device as u8 + ADDRESS, address);
    }
}

#[inline(always)]
fn dispatch(index: usize, frame: &InterruptStackFrame) {
    let _trap_time = crate::task::thread::TrapTime::enter(frame.code_segment.0 & 3 == 3);
    crate::per_cpu::irq_enter();
    let handler = HANDLERS[index].load(Ordering::Acquire);
    if handler != 0 {
        // SAFETY: a nonzero entry was stored from a `fn()` by `enable`.
        let handler: fn() = unsafe { core::mem::transmute(handler) };
        handler();
    }
    super::apic::eoi();
    crate::per_cpu::irq_exit();
}

macro_rules! entries {
    ($($index:literal),*) => {
        /// The IDT gate for each MSI vector, in vector order.
        pub const ENTRIES: [extern "x86-interrupt" fn(InterruptStackFrame); VECTORS] = [$({
            extern "x86-interrupt" fn entry(frame: InterruptStackFrame) {
                dispatch($index, &frame);
            }
            entry
        }),*];
    };
}

entries!(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15);
