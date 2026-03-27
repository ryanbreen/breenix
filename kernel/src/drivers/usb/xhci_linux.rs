//! Linux xHCI C harness integration (feature-gated).

#![cfg(feature = "xhci_linux_harness")]

use crate::drivers::pci::Device;
use crate::drivers::usb::xhci::{xhci_trace_dump_public, xhci_trace_raw, xhci_trace_set_active};

#[repr(C)]
pub struct LinuxXhciState {
    pub base: u64,
    pub op_base: u64,
    pub rt_base: u64,
    pub db_base: u64,
    pub cap_length: u8,
    pub max_slots: u8,
    pub max_ports: u8,
    pub context_size: u8,
}

extern "C" {
    fn linux_xhci_init(state: *mut LinuxXhciState) -> i32;
}

#[inline]
fn read32(addr: u64) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

/// Entry point for the Linux xHCI C harness.
///
/// This is intentionally minimal: it maps the controller registers, builds a
/// small state struct, and delegates to the C harness for all logic.
pub fn init(pci_dev: &Device) -> Result<(), &'static str> {
    xhci_trace_set_active(true);
    xhci_trace_raw(50, 0, 0, b"linux_harness_init");

    pci_dev.enable_bus_master();
    pci_dev.enable_memory_space();

    let bar = pci_dev.get_mmio_bar().ok_or("XHCI: no MMIO BAR found")?;
    let base = crate::arch_impl::aarch64::constants::HHDM_BASE + bar.address;

    let cap_word = read32(base);
    let cap_length = (cap_word & 0xFF) as u8;
    let hcsparams1 = read32(base + 0x04);
    let hccparams1 = read32(base + 0x10);
    let db_offset = read32(base + 0x14) & !0x3u32;
    let rts_offset = read32(base + 0x18) & !0x1Fu32;

    let max_slots = (hcsparams1 & 0xFF) as u8;
    let max_ports = ((hcsparams1 >> 24) & 0xFF) as u8;
    let context_size = if hccparams1 & (1 << 2) != 0 { 64 } else { 32 };

    let op_base = base + cap_length as u64;
    let rt_base = base + rts_offset as u64;
    let db_base = base + db_offset as u64;

    let mut state = LinuxXhciState {
        base,
        op_base,
        rt_base,
        db_base,
        cap_length,
        max_slots,
        max_ports,
        context_size,
    };

    let rc = unsafe { linux_xhci_init(&mut state as *mut LinuxXhciState) };
    xhci_trace_raw(50, 0, 0, b"linux_harness_done");
    xhci_trace_dump_public();
    xhci_trace_set_active(false);

    if rc == 0 {
        Ok(())
    } else {
        Err("XHCI linux harness init failed")
    }
}

#[no_mangle]
pub extern "C" fn breenix_xhci_trace_raw_c(op: u8, slot: u8, dci: u8, data: *const u8, len: usize) {
    if data.is_null() || len == 0 {
        return;
    }
    let bytes = unsafe { core::slice::from_raw_parts(data, len) };
    xhci_trace_raw(op, slot, dci, bytes);
}

#[no_mangle]
pub extern "C" fn breenix_xhci_trace_note_c(slot: u8, data: *const u8, len: usize) {
    if data.is_null() || len == 0 {
        return;
    }
    let bytes = unsafe { core::slice::from_raw_parts(data, len) };
    xhci_trace_raw(50, slot, 0, bytes);
}
